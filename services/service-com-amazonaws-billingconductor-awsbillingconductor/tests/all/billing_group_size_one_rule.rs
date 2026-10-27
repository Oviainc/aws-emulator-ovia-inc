// SPDX-License-Identifier: CC0-1.0

//! `Size` — "The number of accounts in the particular billing group" — across the two
//! operations that report it, end to end over the service's real `restJson1` wire through
//! [`MultiServiceRouter::dispatch`], the same path the production router serves.
//!
//! RULE: ONE rule answers `Size` for every reader of a billing group — the accounts whose
//! membership of that group is in force in the billing period asked about — so the number
//! `UpdateBillingGroup` answers with and the one the read-only `ListBillingGroups` reports
//! for the same group in the same period are the SAME number, whatever the membership and
//! whatever the update changed.
//!
//! The rows differ along what the rule generalises over: how many accounts the group was
//! created with (none, one, several, the modelled maximum of `AccountIdList`), whether one
//! of them is the group's primary account, how many accounts an `AssociateAccounts` brought
//! in afterwards, and how many a `DisassociateAccounts` released again — and each row drives
//! the update twice, once with a member for it to change and once with nothing to change,
//! because only one of those two writes a generation at all.
//!
//! `tests/all/account_association_billing_periods.rs` owns the per-period dimension of the
//! same count; this suite is about the two readers agreeing on it.

use serde_json::{json, Value};

use crate::billing_group_billing_periods::{current_period, Emulator};

/// `POST /create-billing-group`, which gives a row a group to count the accounts of. The
/// shared harness always designates a primary account, and a row here needs a group with
/// none, so this suite builds the create body itself.
const CREATE_GROUP_PATH: &str = "/create-billing-group";

/// `POST /update-billing-group` — the first of the two readers under test.
const UPDATE_GROUP_PATH: &str = "/update-billing-group";

/// `POST /list-billing-groups` — the second, read-only reader under test.
const LIST_GROUPS_PATH: &str = "/list-billing-groups";

/// The membership writes a row moves the count with, at the methods their modelled HTTP
/// bindings name (`smithy_query(query="operation",operation="AssociateAccounts")`: a
/// `POST`).
const ASSOCIATE_PATH: &str = "/associate-accounts";
const DISASSOCIATE_PATH: &str = "/disassociate-accounts";

/// The modelled maximum length of `AccountIdList`, the list `AccountGrouping`'s
/// `LinkedAccountIds` and `AssociateAccounts`' `AccountIds` are both spelled with
/// (`smithy_query(query="shape",shape="AccountIdList")`: `len 0..30`).
const MAX_ACCOUNT_IDS: usize = 30;

/// The `index`-th account of the `row`-th row, as the modelled `AccountId` pattern spells
/// one (`^[0-9]{12}$`). One block of ids per row, because an account belongs to at most one
/// billing group.
fn account_id(row: usize, index: usize) -> String {
    format!("{:012}", 777_700_000_000_u64 + (row as u64) * 100 + index as u64)
}

/// One row of the table: the membership the group ends up with, assembled out of the three
/// writes that can build one.
struct Row {
    label: &'static str,
    /// How many accounts the create lists.
    created: usize,
    /// Is the first created account designated the group's primary account? A group with
    /// no billing owner is served too, and counts its accounts the same way.
    primary: bool,
    /// How many further accounts an `AssociateAccounts` brings in.
    associated: usize,
    /// How many of the created accounts a `DisassociateAccounts` releases again, taken from
    /// the end of the created block so the primary account is never among them ("the
    /// primary account of a billing group cannot be disassociated").
    released: usize,
}

impl Row {
    /// The accounts the create lists.
    fn created_ids(&self, row: usize) -> Vec<String> {
        (0..self.created).map(|index| account_id(row, index)).collect()
    }

    /// The accounts the association brings in, disjoint from the created block.
    fn associated_ids(&self, row: usize) -> Vec<String> {
        (0..self.associated)
            .map(|index| account_id(row, MAX_ACCOUNT_IDS + index))
            .collect()
    }

    /// The accounts the disassociation releases: the last ones of the created block, so
    /// index 0 — the primary account when the row designates one — stays a member.
    fn released_ids(&self, row: usize) -> Vec<String> {
        let created = self.created_ids(row);
        created[created.len() - self.released..].to_vec()
    }

    /// The count both readers must report once the row's writes are served.
    fn expected_size(&self) -> i64 {
        i64::try_from(self.created + self.associated - self.released).unwrap_or(i64::MAX)
    }
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "no accounts and no primary account at all",
            created: 0,
            primary: false,
            associated: 0,
            released: 0,
        },
        Row {
            label: "one account, which is the primary",
            created: 1,
            primary: true,
            associated: 0,
            released: 0,
        },
        Row {
            label: "two accounts at create, no primary designated",
            created: 2,
            primary: false,
            associated: 0,
            released: 0,
        },
        Row {
            label: "one account at create and three brought in by an association",
            created: 1,
            primary: true,
            associated: 3,
            released: 0,
        },
        Row {
            label: "three accounts at create, two of them released again",
            created: 3,
            primary: true,
            associated: 0,
            released: 2,
        },
        Row {
            label: "accounts associated and released in the same billing period",
            created: 2,
            primary: true,
            associated: 3,
            released: 1,
        },
        Row {
            label: "the modelled maximum account list at create",
            created: MAX_ACCOUNT_IDS,
            primary: true,
            associated: 0,
            released: 0,
        },
        Row {
            label: "the modelled maximum brought in by one association",
            created: 1,
            primary: true,
            associated: MAX_ACCOUNT_IDS,
            released: 0,
        },
    ]
}

/// `UpdateBillingGroup` against `group` carrying `patch` beside the ARN: the status and the
/// `Size` it answers with.
async fn update_size(em: &Emulator, case: &str, group: &str, patch: &Value) -> i64 {
    let mut body = patch.clone();
    body["Arn"] = json!(group);
    let (status, answer) = em.post(UPDATE_GROUP_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateBillingGroup to be served with 200, got {status} \
         with {answer:?}"
    );
    answer["Size"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no Size in the update's answer {answer:?}"))
}

/// The `Size` `ListBillingGroups` reports for `group` in `period` (`None` names no period
/// at all, which asks about the current one).
async fn listed_size(em: &Emulator, case: &str, group: &str, period: Option<&str>) -> i64 {
    let mut body = json!({"Filters": {"Arns": [group]}});
    if let Some(period) = period {
        body["BillingPeriod"] = json!(period);
    }
    let (status, answer) = em.post(LIST_GROUPS_PATH, &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListBillingGroups to be served with 200, got {status} \
         with {answer:?}"
    );
    let element = answer["BillingGroups"]
        .as_array()
        .and_then(|groups| groups.first())
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the group is not reported at all in {answer:?}"));
    element["Size"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no Size in the listed element {element:?}"))
}

/// Write a membership through the wire, asserting it was served.
async fn membership_write(em: &Emulator, case: &str, path: &str, group: &str, accounts: &[String]) {
    if accounts.is_empty() {
        return;
    }
    let (status, answer) = em
        .post(path, &json!({"Arn": group, "AccountIds": accounts}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: setup {path} expected 200, got {status} with {answer:?}"
    );
}

#[tokio::test]
async fn the_update_and_the_listing_report_the_same_size_for_the_same_group() {
    let em = Emulator::new();
    let plan = em.create_plan("SizeOneRulePlan").await;
    let now = current_period();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.label;
        let created = row.created_ids(index);
        let mut body = json!({
            "Name": format!("SizeOneRule-{index}"),
            "AccountGrouping": {"LinkedAccountIds": created},
            "ComputationPreference": {"PricingPlanArn": plan},
        });
        if row.primary {
            body["PrimaryAccountId"] = json!(created
                .first()
                .expect("a row designating a primary account creates at least one account"));
        }
        let (status, answer) = em.post(CREATE_GROUP_PATH, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBillingGroup expected 200, got {status} with {answer:?}"
        );
        let group = answer["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Arn in {answer:?}"))
            .to_string();

        membership_write(
            &em,
            case,
            ASSOCIATE_PATH,
            &group,
            &row.associated_ids(index),
        )
        .await;
        membership_write(
            &em,
            case,
            DISASSOCIATE_PATH,
            &group,
            &row.released_ids(index),
        )
        .await;

        let expected = row.expected_size();
        // The update twice: once with a member for it to change, once with nothing to
        // change, since only the first of them writes a generation at all.
        let patches = [
            ("a member to change", json!({"Description": case})),
            ("nothing to change", json!({})),
        ];
        for (patch_label, patch) in patches {
            let case = format!("{case}, the update carrying {patch_label}");
            let answered = update_size(&em, &case, &group, &patch).await;
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the update to report Size {expected}, got {answered}"
            );
            // The same number out of the other reader, for the period this call was made
            // in — named explicitly and left unnamed, which asks about that same period.
            for period in [None, Some(now.as_str())] {
                let case = format!("{case}, listed at {period:?}");
                let listed = listed_size(&em, &case, &group, period).await;
                assert_eq!(
                    listed, expected,
                    "case {case:?}: expected the listing to report Size {expected}, got {listed}"
                );
                assert_eq!(
                    listed, answered,
                    "case {case:?}: the two readers of one group must report one Size, the \
                     update answered {answered} and the listing {listed}"
                );
            }
        }
    }
}
