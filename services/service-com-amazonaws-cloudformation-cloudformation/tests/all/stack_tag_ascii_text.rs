// SPDX-License-Identifier: CC0-1.0

//! The TEXT of a tag a caller puts on a stack, a change set or a stack set, over the awsQuery
//! wire.
//!
//! **The rule every case here holds the service to: CloudFormation holds a caller's tag text
//! as ASCII, so a tag reads back with every codepoint outside ASCII replaced by exactly one
//! `?` and every ASCII character, the character count and the order of the list left as sent —
//! whichever operation wrote the tag and whichever operation reads it back.**
//!
//! `Tag$Key` is "a string used to identify this tag" of 1-128 characters and `Tag$Value` "a
//! string that contains the value for this tag" of 1-256
//! (`smithy_query(query="shape", shape="Tag")`): neither member constrains the CHARACTER SET,
//! so a request may send any text within those bounds and the service answers 200 — what the
//! store cannot hold is the codepoint, not the request.
//!
//! The rows span what the rule generalises over: the width of the encoding (a Latin-1 letter,
//! a two-, three- and four-byte encoding — one `?` per codepoint, not per byte), where the
//! non-ASCII text sits (the whole value, mixed into ASCII text, in the tag's KEY), the ASCII
//! text that must survive untouched (the printable punctuation a tag value carries and the
//! first and last ASCII codepoints), the length (a value at the published 256-character
//! maximum, which the replacement keeps admissible because it is one character for one), the
//! size of the list (one tag, and several whose order is kept), and the FIVE operations whose
//! input carries a `Tags` list, each read back through the operation that reports its tags.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, wait_for_the_transition, xml_blocks,
    xml_text,
};

/// The template every case here deploys: one topic, really created in Amazon SNS.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"ascii-tag-text"}}}}"#;

/// A second revision of it, so an UPDATE has something to change.
const REVISED: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
     "Properties":{"DisplayName":"ascii-tag-text-revised"}}}}"#;

const STACK: &str = "ascii-tag-text";
const STACK_SET: &str = "ascii-tag-text-set";
const CHANGE_SET: &str = "ascii-tag-text-plan";

/// One row of the tag-text table: the tags a request sends, and the tags a read must report
/// for them.
struct TextCase {
    case: &'static str,
    /// The `(Key, Value)` pairs the request carries, in request order.
    sent: Vec<(String, String)>,
    /// The `(Key, Value)` pairs a read must report, in the same order.
    held: Vec<(String, String)>,
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn text_cases() -> Vec<TextCase> {
    // The published maximum of `Tag$Value` is 256 characters, so a value of 256 non-ASCII
    // codepoints is still a value the bound admits once each is replaced one-for-one.
    let at_the_maximum: String = "\u{65e5}".repeat(256);
    let at_the_maximum_held: String = "?".repeat(256);
    vec![
        TextCase {
            case: "wholly ASCII text, with the punctuation a tag value carries",
            sent: pairs(&[("Owner", "team-1/build+v1.0:release-2024@example")]),
            held: pairs(&[("Owner", "team-1/build+v1.0:release-2024@example")]),
        },
        TextCase {
            case: "the first and the last ASCII codepoint, which are held as they are",
            sent: pairs(&[("Owner", "a\u{1}b\u{7f}c")]),
            held: pairs(&[("Owner", "a\u{1}b\u{7f}c")]),
        },
        TextCase {
            case: "the first codepoint past ASCII",
            sent: pairs(&[("Owner", "caf\u{80}")]),
            held: pairs(&[("Owner", "caf?")]),
        },
        TextCase {
            case: "a Latin-1 letter inside ASCII text",
            sent: pairs(&[("Owner", "Bj\u{f6}rk")]),
            held: pairs(&[("Owner", "Bj?rk")]),
        },
        TextCase {
            case: "a two-byte encoding outside Latin-1",
            sent: pairs(&[("Owner", "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}")]),
            held: pairs(&[("Owner", "??????")]),
        },
        TextCase {
            case: "a three-byte encoding, one question mark per codepoint",
            sent: pairs(&[("Owner", "\u{65e5}\u{672c}\u{8a9e}\u{30bf}\u{30b0}")]),
            held: pairs(&[("Owner", "?????")]),
        },
        TextCase {
            case: "a four-byte encoding, one codepoint and so one question mark",
            sent: pairs(&[("Owner", "\u{1f642}")]),
            held: pairs(&[("Owner", "?")]),
        },
        TextCase {
            case: "non-ASCII text mixed into ASCII text, which survives around it",
            sent: pairs(&[(
                "Owner",
                "\u{65e5}\u{672c}\u{8a9e}\u{30bf}\u{30b0} Bj\u{f6}rk \
                 caf\u{e9}_100:build/v1.0+release-2024@example",
            )]),
            held: pairs(&[(
                "Owner",
                "????? Bj?rk caf?_100:build/v1.0+release-2024@example",
            )]),
        },
        TextCase {
            case: "non-ASCII text in the tag's KEY, which is held by the same rule",
            sent: pairs(&[("Propri\u{e9}taire", "platform")]),
            held: pairs(&[("Propri?taire", "platform")]),
        },
        TextCase {
            case: "a value at the published 256-character maximum, wholly non-ASCII",
            sent: vec![("Owner".to_string(), at_the_maximum)],
            held: vec![("Owner".to_string(), at_the_maximum_held)],
        },
        TextCase {
            case: "several tags, each held by the rule and the list's order kept",
            sent: pairs(&[
                ("tier", "s\u{ed}lver"),
                ("team", "d\u{e1}ta"),
                ("owner", "platform"),
            ]),
            held: pairs(&[
                ("tier", "s?lver"),
                ("team", "d?ta"),
                ("owner", "platform"),
            ]),
        },
    ]
}

/// The `Tags.member.N.Key` / `.Value` members one tag list is sent as.
fn tag_members(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        members.push(member(&format!("Tags.member.{}.Key", index + 1), key));
        members.push(member(&format!("Tags.member.{}.Value", index + 1), value));
    }
    members
}

/// The `(Key, Value)` pairs one response's `Tags` list reports, in wire order.
///
/// Read INSIDE the `Tags` block, so no other list's members can be swept up with it.
fn reported_tags(body: &str) -> Vec<(String, String)> {
    let Some(block) = xml_blocks(body, "Tags").into_iter().next() else {
        return Vec::new();
    };
    xml_blocks(&block, "member")
        .into_iter()
        .map(|entry| {
            (
                xml_text(&entry, "Key").unwrap_or_default(),
                xml_text(&entry, "Value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The operations whose input carries a caller's `Tags` list — every one this service models,
/// so the rule is asserted over the whole surface that stores a tag.
#[derive(Debug, Clone, Copy)]
enum TakesTags {
    CreateStack,
    UpdateStack,
    CreateChangeSet,
    CreateStackSet,
    UpdateStackSet,
}

const TAKES_TAGS: &[TakesTags] = &[
    TakesTags::CreateStack,
    TakesTags::UpdateStack,
    TakesTags::CreateChangeSet,
    TakesTags::CreateStackSet,
    TakesTags::UpdateStackSet,
];

/// The stack a row that mutates one acts on, created with no tags and settled.
async fn settled_stack(case: &str, router: &MultiServiceRouter) {
    let (status_code, body) = create_stack(router, STACK, TEMPLATE, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the stack under test must be created, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    wait_for_the_transition().await;
}

/// The stack set a row that mutates one acts on, created with no tags.
async fn stack_set(case: &str, router: &MultiServiceRouter) {
    let (status_code, body) = call(
        router,
        "CreateStackSet",
        &[
            member("StackSetName", STACK_SET),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: the stack set under test must be created, got {status_code} {body}"
    );
}

/// Send `tags` as the `Tags` of `op` on a fresh router, then read the tags back through the
/// operation that reports the record `op` wrote.
async fn round_trip(case: &str, op: TakesTags, tags: &[(String, String)]) -> Vec<(String, String)> {
    let router = router();
    let sent = tag_members(tags);
    let (status_code, body) = match op {
        TakesTags::CreateStack => create_stack(&router, STACK, TEMPLATE, &sent).await,
        TakesTags::UpdateStack => {
            settled_stack(case, &router).await;
            let mut members = vec![member("StackName", STACK), member("TemplateBody", REVISED)];
            members.extend(sent);
            call(&router, "UpdateStack", &members).await
        }
        TakesTags::CreateChangeSet => {
            settled_stack(case, &router).await;
            let mut members = vec![
                member("StackName", STACK),
                member("ChangeSetName", CHANGE_SET),
                member("TemplateBody", REVISED),
            ];
            members.extend(sent);
            call(&router, "CreateChangeSet", &members).await
        }
        TakesTags::CreateStackSet => {
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("TemplateBody", TEMPLATE),
            ];
            members.extend(sent);
            call(&router, "CreateStackSet", &members).await
        }
        TakesTags::UpdateStackSet => {
            stack_set(case, &router).await;
            let mut members = vec![
                member("StackSetName", STACK_SET),
                member("TemplateBody", REVISED),
            ];
            members.extend(sent);
            call(&router, "UpdateStackSet", &members).await
        }
    };
    assert_eq!(
        status_code, 200,
        "case {op:?} {case:?}: the model constrains no tag CHARACTER SET, so the call must be \
         admitted, got {status_code} {body}"
    );
    let (status_code, read) = match op {
        TakesTags::CreateStack | TakesTags::UpdateStack => {
            // The update settles before the read, so what is reported is the stack's own
            // record rather than one still being written.
            wait_for_the_transition().await;
            describe_stack(&router, STACK).await
        }
        TakesTags::CreateChangeSet => {
            call(
                &router,
                "DescribeChangeSet",
                &[
                    member("ChangeSetName", CHANGE_SET),
                    member("StackName", STACK),
                ],
            )
            .await
        }
        TakesTags::CreateStackSet | TakesTags::UpdateStackSet => {
            call(
                &router,
                "DescribeStackSet",
                &[member("StackSetName", STACK_SET)],
            )
            .await
        }
    };
    assert_eq!(
        status_code, 200,
        "case {op:?} {case:?}: the record must be readable, got {status_code} {read}"
    );
    reported_tags(&read)
}

#[tokio::test]
async fn a_stacks_tag_text_is_held_as_ascii_and_read_back_that_way() {
    for TextCase { case, sent, held } in text_cases() {
        let reported = round_trip(case, TakesTags::CreateStack, &sent).await;
        assert_eq!(
            reported, held,
            "case {case:?}: expected the tags held as ASCII, got {reported:?}"
        );
        for ((_, sent_value), (_, held_value)) in sent.iter().zip(reported.iter()) {
            assert_eq!(
                held_value.chars().count(),
                sent_value.chars().count(),
                "case {case:?}: expected one character held per character sent, got \
                 {held_value:?} for {sent_value:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_operation_that_takes_tags_holds_their_text_as_ascii() {
    // One row of the table, driven through all five writers and read back through the
    // operation that reports each record's tags: no writer may hold a tag differently from
    // the others.
    let case = "non-ASCII text in both the key and the value";
    let sent = pairs(&[
        ("Propri\u{e9}taire", "Bj\u{f6}rk"),
        ("team", "\u{65e5}\u{672c}\u{8a9e}"),
    ]);
    let held = pairs(&[("Propri?taire", "Bj?rk"), ("team", "???")]);
    for op in TAKES_TAGS {
        let reported = round_trip(case, *op, &sent).await;
        assert_eq!(
            reported, held,
            "case {op:?} {case:?}: expected the tags held as ASCII, got {reported:?}"
        );
    }
}
