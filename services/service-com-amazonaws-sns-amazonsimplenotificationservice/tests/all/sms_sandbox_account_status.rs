// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetSMSSandboxAccountStatus`, the SMS sandbox ACCOUNT-level
//! read.
//!
//! The rule under test: the operation reports the sandbox membership the calling
//! account's persisted record holds — an account that has just started using SNS for
//! SMS is in the sandbox — and reporting it changes nothing, so neither the read
//! itself nor anything a caller does to the sandbox's destination numbers moves the
//! membership.
//!
//! The rows below differ in the membership VALUE the account record holds, the number
//! of destination numbers the sandbox holds, the verification STATUS those numbers
//! reached, the destination-number FORM, and how many times the status is read — which
//! is what the rule generalises over. Every call goes over the wire through the router
//! the emulator serves in production, so nothing but persisted state carries
//! information between calls.

use service_com_amazonaws_sns_amazonsimplenotificationservice::state::{
    IN_SMS_SANDBOX_AT_ACCOUNT_START, SANDBOX_NUMBER_PENDING, SANDBOX_NUMBER_VERIFIED,
};

use super::sms_sandbox_phone_numbers::{accepted_number_forms, add, reported_numbers};
use super::sms_sandbox_verification::{delivered_code, verify};
use super::topics_subscriptions_publish::Sns;

/// The account status as the wire reports it: the text of the required
/// `IsInSandbox` member of an answered call.
///
/// The member is modelled `required` with a boolean target
/// (`smithy_query(query="operation", operation="GetSMSSandboxAccountStatus")` reports
/// `IsInSandbox {target: boolean, required: true}`), so an answered call always
/// carries it, spelled as a boolean literal.
async fn reported_status(sns: &Sns, case: &str) -> bool {
    let answer = sns.call("GetSMSSandboxAccountStatus", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status, answer.body
    );
    let reported = answer
        .element("IsInSandbox")
        .unwrap_or_else(|| panic!("case {case:?}: no IsInSandbox member in {}", answer.body));
    match reported.as_str() {
        "true" => true,
        "false" => false,
        other => panic!("case {case:?}: expected a boolean literal, got {other:?}"),
    }
}

/// Put `is_in_sandbox` into the account's persisted record, the way the state the
/// emulator boots with holds it.
fn seed_membership(sns: &Sns, is_in_sandbox: bool) {
    let state = sns.state();
    state.lock().sms_sandbox_account.is_in_sandbox = is_in_sandbox;
}

/// A freshly booted account is in the SMS sandbox, and reading that says so however
/// many times it is read.
#[tokio::test]
async fn a_fresh_account_is_reported_in_the_sms_sandbox() {
    let sns = Sns::new();

    for read in 0..3 {
        let case = format!("read {read} of a fresh account");
        assert_eq!(
            reported_status(&sns, &case).await,
            IN_SMS_SANDBOX_AT_ACCOUNT_START,
            "case {case:?}: expected the membership an account starts with"
        );
    }
}

/// The report is the membership the account's record HOLDS — both values — and a
/// read neither joins nor leaves the sandbox: repeated reads report the same thing and
/// leave the record and the sandbox's destination numbers alone.
#[tokio::test]
async fn the_report_is_the_membership_the_account_record_holds() {
    for held in [true, false] {
        let sns = Sns::new();
        seed_membership(&sns, held);
        add(&sns, "a held destination number", "+12065550100").await;

        for read in 0..3 {
            let case = format!("IsInSandbox={held} / read {read}");
            assert_eq!(
                reported_status(&sns, &case).await,
                held,
                "case {case:?}: expected the record's membership"
            );
            let state = sns.state();
            let state = state.lock();
            assert_eq!(
                state.sms_sandbox_account.is_in_sandbox, held,
                "case {case:?}: the read must not move the membership, got {:?}",
                state.sms_sandbox_account
            );
            assert_eq!(
                state.sandbox_phone_numbers.len(),
                1,
                "case {case:?}: the read must not add or drop a destination number"
            );
        }
    }
}

/// The destination numbers do not decide the membership: adding a number, verifying
/// it and deleting it again leave the reported status exactly as the account's record
/// holds it, for either value of that record and whatever the sandbox holds.
#[tokio::test]
async fn the_destination_numbers_do_not_decide_the_membership() {
    for held in [true, false] {
        let sns = Sns::new();
        seed_membership(&sns, held);

        let case = format!("IsInSandbox={held} / an empty sandbox");
        assert_eq!(
            reported_status(&sns, &case).await,
            held,
            "case {case:?}: expected the record's membership"
        );

        let forms = accepted_number_forms();
        for (form, number) in &forms {
            add(&sns, form, number).await;
            let case = format!("IsInSandbox={held} / {form:?} pending");
            assert_eq!(
                reported_status(&sns, &case).await,
                held,
                "case {case:?}: a pending destination number must not move the membership"
            );

            let code = delivered_code(&sns, number);
            let verified = verify(&sns, number, &code).await;
            assert_eq!(
                verified.status, 200,
                "case {case:?}: expected the verification to answer 200, got {} / {}",
                verified.status, verified.body
            );
            let case = format!("IsInSandbox={held} / {form:?} verified");
            assert_eq!(
                reported_status(&sns, &case).await,
                held,
                "case {case:?}: a verified destination number must not move the membership"
            );
        }

        // Every number is now verified, so the sandbox is in the state a caller
        // reaches just before asking for production access — and that ask is no SNS
        // operation, so the membership is still the record's.
        let listed = sns.call("ListSMSSandboxPhoneNumbers", &[]).await;
        let statuses: Vec<String> = reported_numbers(&listed)
            .into_iter()
            .map(|(_, status)| status)
            .collect();
        assert_eq!(
            statuses,
            vec![SANDBOX_NUMBER_VERIFIED.to_string(); forms.len()],
            "IsInSandbox={held}: expected every number verified, got {statuses:?}"
        );
        assert_ne!(
            SANDBOX_NUMBER_VERIFIED, SANDBOX_NUMBER_PENDING,
            "the two modelled verification statuses must differ"
        );

        for (form, number) in &forms {
            let deleted = sns
                .call("DeleteSMSSandboxPhoneNumber", &[("PhoneNumber", number)])
                .await;
            assert_eq!(
                deleted.status, 200,
                "IsInSandbox={held} / {form:?}: expected the delete to answer 200, got {} / {}",
                deleted.status, deleted.body
            );
        }
        let case = format!("IsInSandbox={held} / every number deleted");
        assert_eq!(
            reported_status(&sns, &case).await,
            held,
            "case {case:?}: emptying the sandbox must not move the membership"
        );
    }
}
