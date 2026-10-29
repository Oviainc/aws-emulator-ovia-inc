// SPDX-License-Identifier: CC0-1.0

//! A bound a Storage Lens group's object-size or object-age condition was NOT given
//! stays absent through write → store → read.
//!
//! The rule under test: a member the request did not carry is not materialised on the
//! response, so the document a read reports carries exactly the members the document it
//! was written with carried — a member the model gives a `@default` included. The two
//! conditions are where that bites: `MatchObjectSize$BytesGreaterThan` / `BytesLessThan`
//! and `MatchObjectAge$DaysGreaterThan` / `DaysLessThan` each carry a modelled
//! `@default` of 0, and 0 is a value their own documentation excludes ("must be a
//! positive number, greater than 0"; an upper bound must be "greater than the minimum
//! object size"), so reporting it for a bound nobody set states a range the group does
//! not filter on — and a client that reads a group, edits one clause and writes it back
//! would then submit it.
//!
//! Every row differs along something that rule generalises over: which condition carries
//! a partial range, which of its two bounds the document carries, whether the condition
//! sits directly below `Filter` or below its `And` / `Or` operator, how many other
//! criteria stand beside it, and whether the group is written by `CreateStorageLensGroup`
//! or replaced by `UpdateStorageLensGroup`. Two rows are controls: one carrying BOTH
//! bounds (both are reported) and one carrying a bound submitted AS 0 (it is reported as
//! 0), so what decides is whether the member was sent and not what its value is.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it per the
//! Smithy model, runs the handler and serializes the restXml response.

use crate::access_point_lifecycle::{Emulator, ACCOUNT, REGION};
use crate::storage_lens_configuration::{leaf, sorted, sorted_leaves, Leaf};
use crate::storage_lens_group::{create_group, get_group, Group};
use crate::storage_lens_group_update::update_body;

/// The bound element names the two conditions carry, from
/// `smithy_query(query="shape",shape="MatchObjectSize,MatchObjectAge")`: every one of
/// them is a member a document may leave out, and so an element a read must not invent.
const BOUND_ELEMENTS: [&str; 4] = [
    "BytesGreaterThan",
    "BytesLessThan",
    "DaysGreaterThan",
    "DaysLessThan",
];

/// The documents under test, each with the bound elements it must report.
fn rows() -> Vec<(Group, Vec<&'static str>)> {
    vec![
        (
            Group {
                case: "a size condition with only its lower bound",
                leaves: vec![leaf("Filter/MatchObjectSize/BytesGreaterThan", "1024")],
            },
            vec!["BytesGreaterThan"],
        ),
        (
            Group {
                case: "a size condition with only its upper bound, past 32 bits",
                leaves: vec![leaf("Filter/MatchObjectSize/BytesLessThan", "5368709120")],
            },
            vec!["BytesLessThan"],
        ),
        (
            Group {
                case: "an age condition with only its lower bound",
                leaves: vec![leaf("Filter/MatchObjectAge/DaysGreaterThan", "30")],
            },
            vec!["DaysGreaterThan"],
        ),
        (
            Group {
                case: "an age condition with only its upper bound",
                leaves: vec![leaf("Filter/MatchObjectAge/DaysLessThan", "365")],
            },
            vec!["DaysLessThan"],
        ),
        (
            Group {
                case: "an And of a prefix and a size condition with only its lower bound",
                leaves: vec![
                    leaf("Filter/And/MatchAnyPrefix/Prefix", "logs/"),
                    leaf("Filter/And/MatchObjectSize/BytesGreaterThan", "1024"),
                ],
            },
            vec!["BytesGreaterThan"],
        ),
        (
            Group {
                case: "an Or of a suffix and an age condition with only its upper bound",
                leaves: vec![
                    leaf("Filter/Or/MatchAnySuffix/Suffix", ".tmp"),
                    leaf("Filter/Or/MatchObjectAge/DaysLessThan", "7"),
                ],
            },
            vec!["DaysLessThan"],
        ),
        (
            Group {
                case: "an And whose size and age conditions each carry one bound",
                leaves: vec![
                    leaf("Filter/And/MatchAnyTag/Tag/Key", "tier"),
                    leaf("Filter/And/MatchAnyTag/Tag/Value", "gold"),
                    leaf("Filter/And/MatchObjectAge/DaysGreaterThan", "1"),
                    leaf("Filter/And/MatchObjectSize/BytesLessThan", "1073741824"),
                ],
            },
            vec!["DaysGreaterThan", "BytesLessThan"],
        ),
        (
            // Control: a condition carrying BOTH bounds reports both.
            Group {
                case: "a size condition carrying both bounds",
                leaves: vec![
                    leaf("Filter/MatchObjectSize/BytesGreaterThan", "1024"),
                    leaf("Filter/MatchObjectSize/BytesLessThan", "2048"),
                ],
            },
            vec!["BytesGreaterThan", "BytesLessThan"],
        ),
        (
            // Control: a bound SUBMITTED as 0 comes back as 0. What decides whether a
            // member is reported is whether the request carried it, not its value.
            Group {
                case: "a size condition whose lower bound was submitted as zero",
                leaves: vec![
                    leaf("Filter/MatchObjectSize/BytesGreaterThan", "0"),
                    leaf("Filter/MatchObjectSize/BytesLessThan", "4096"),
                ],
            },
            vec!["BytesGreaterThan", "BytesLessThan"],
        ),
    ]
}

/// Assert that `name`'s stored document reads back as exactly `group`'s, carrying
/// `reported` bound elements and no others.
async fn assert_reads_back(
    emulator: &Emulator,
    name: &str,
    group: &Group,
    reported: &[&str],
    written_by: &str,
) {
    let case = group.case;
    let read = get_group(emulator, ACCOUNT, name).await;
    assert_eq!(
        read.status, 200,
        "case {case:?} written by {written_by}: expected 200 from the read, got {} {}",
        read.status, read.body
    );
    assert_eq!(
        sorted_leaves(&read.body),
        sorted(&group.expected(name, REGION)),
        "case {case:?} written by {written_by}: the read must report exactly the \
         document that was written, got {}",
        read.body
    );
    for element in BOUND_ELEMENTS {
        let found = read.texts(element);
        if reported.contains(&element) {
            assert_eq!(
                found.len(),
                group
                    .leaves
                    .iter()
                    .filter(|(path, _)| path.split('/').next_back() == Some(element))
                    .count(),
                "case {case:?} written by {written_by}: expected {element} once per \
                 submitted bound, got {found:?} in {}",
                read.body
            );
        } else {
            assert!(
                found.is_empty(),
                "case {case:?} written by {written_by}: {element} was never submitted \
                 and must not be reported, got {found:?} in {}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn a_bound_a_created_group_never_carried_is_not_reported_back() {
    let emulator = Emulator::start();

    for (index, (group, reported)) in rows().iter().enumerate() {
        let name = format!("created-{index}");
        let created = create_group(&emulator, ACCOUNT, &name, group, None).await;
        assert_eq!(
            created.status, 204,
            "case {:?}: expected the create to be accepted with 204, got {} {}",
            group.case, created.status, created.body
        );
        assert_reads_back(&emulator, &name, group, reported, "a create").await;
    }
}

#[tokio::test]
async fn a_bound_an_updated_group_never_carried_is_not_reported_back() {
    let emulator = Emulator::start();

    // Every group starts from a document whose conditions carry BOTH of their bounds, so
    // a bound the read reports after the update could only have come from the update's
    // own document — or been invented.
    let full = Group {
        case: "both bounds of both conditions",
        leaves: vec![
            leaf("Filter/And/MatchObjectAge/DaysGreaterThan", "2"),
            leaf("Filter/And/MatchObjectAge/DaysLessThan", "3"),
            leaf("Filter/And/MatchObjectSize/BytesGreaterThan", "4"),
            leaf("Filter/And/MatchObjectSize/BytesLessThan", "5"),
        ],
    };

    for (index, (group, reported)) in rows().iter().enumerate() {
        let name = format!("updated-{index}");
        let created = create_group(&emulator, ACCOUNT, &name, &full, None).await;
        assert_eq!(
            created.status, 204,
            "case {:?}: expected the create to be accepted with 204, got {} {}",
            group.case, created.status, created.body
        );

        let submitted: Vec<Leaf> = group.submitted(&name);
        let updated = emulator
            .call(
                "PUT",
                &format!("/v20180820/storagelensgroup/{name}"),
                ACCOUNT,
                &update_body(&submitted),
            )
            .await;
        assert_eq!(
            updated.status, 204,
            "case {:?}: expected the update to be accepted with 204, got {} {}",
            group.case, updated.status, updated.body
        );
        assert_reads_back(&emulator, &name, group, reported, "an update").await;
    }
}
