// SPDX-License-Identifier: CC0-1.0

//! Amazon Simple Notification Service's published default-quota catalog.

use super::*;

/// One Amazon SNS per-API request-rate quota: the transactions per second SNS serves
/// calls of that one API at.
///
/// SNS publishes a rate quota for each of its API actions, and every member of such a row
/// but the action's own name and the value is the same across the class: the window is one
/// second, the usage metric is that action's `AWS/Usage` `CallCount`
/// ([`call_count_metric`]) under the service's `AWS/Usage` display name, and the
/// `Description` is the sentence AWS publishes for a request-rate quota with the action
/// named in it ([`request_rate_description`]). Those are the quota's published members, so
/// one constructor fills them and two rows of the class cannot drift apart.
fn sns_transaction_rate(code: &str, api_name: &str, transactions_per_second: f64) -> QuotaRow {
    quota(
        format!("{api_name} Transactions per Second"),
        transactions_per_second,
    )
    .code(code)
    .period(1, "SECOND")
    .metric(call_count_metric(SNS_USAGE_SERVICE, api_name))
    .describe(&request_rate_description(api_name))
}

/// The name `AWS/Usage` publishes Amazon SNS's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const SNS_USAGE_SERVICE: &str = "SNS";

/// Amazon SNS's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sns` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code sns` reports, which is
/// the "Amazon Simple Notification Service endpoints and quotas" page of the AWS General
/// Reference together with the "Amazon SNS quotas" page of the SNS Developer Guide), and
/// the `QuotaCode`s that catalog is keyed on.
///
/// The catalog is mostly a RATE catalog: SNS publishes the transactions per second it
/// serves each of its API actions at, which is why all but six rows are built by
/// [`sns_transaction_rate`]. The six that are not are the resource planes of the account
/// — its topics, their subscriptions, and the shape of one message — which are counted
/// rather than metered per second and so carry neither a window nor a call count.
pub(in crate::state) fn sns_quotas() -> Vec<QuotaRow> {
    vec![
        sns_transaction_rate("L-01EACE54", "AddPermission", 30.0),
        sns_transaction_rate("L-038DA0E0", "CheckIfPhoneNumberIsOptedOut", 30.0),
        sns_transaction_rate("L-039289D5", "ConfirmSubscription", 100.0),
        sns_transaction_rate("L-154ECCA7", "CreatePlatformApplication", 15.0),
        sns_transaction_rate("L-19F66BE5", "CreatePlatformEndpoint", 15.0),
        sns_transaction_rate("L-1A43D3DB", "CreateSMSSandboxPhoneNumber", 30.0),
        sns_transaction_rate("L-21C7FFA6", "CreateTopic", 30.0),
        sns_transaction_rate("L-2EE2434F", "DeleteEndpoint", 15.0),
        sns_transaction_rate("L-2FE42A3E", "DeletePlatformApplication", 15.0),
        sns_transaction_rate("L-390AADBE", "DeleteSMSSandboxPhoneNumber", 30.0),
        sns_transaction_rate("L-4126E74A", "DeleteTopic", 30.0),
        quota("FIFO Topics per Account", 1000.0)
            .code("L-41B81227")
            .describe(
                "The maximum number of FIFO topics in this account in the current Region.",
            ),
        sns_transaction_rate("L-4659BFC2", "GetDataProtectionPolicy", 30.0),
        sns_transaction_rate("L-4738DDCE", "GetEndpointAttributes", 15.0),
        sns_transaction_rate("L-4EE37BC0", "GetPlatformApplicationAttributes", 15.0),
        sns_transaction_rate("L-61103206", "GetSMSAttributes", 30.0),
        sns_transaction_rate("L-6521CF3A", "GetSMSSandboxAccountStatus", 30.0),
        sns_transaction_rate("L-6635B4C9", "GetSubscriptionAttributes", 30.0),
        sns_transaction_rate("L-6A04D1EB", "GetTopicAttributes", 30.0),
        sns_transaction_rate("L-7B7E3FB8", "ListEndpointsByPlatformApplication", 30.0),
        sns_transaction_rate("L-7E62C088", "ListOriginationNumbers", 30.0),
        sns_transaction_rate("L-820458B0", "ListPhoneNumbersOptedOut", 30.0),
        sns_transaction_rate("L-856966E8", "ListPlatformApplications", 15.0),
        sns_transaction_rate("L-876E1222", "ListSMSSandboxPhoneNumbers", 30.0),
        sns_transaction_rate("L-93308085", "ListSubscriptions", 30.0),
        sns_transaction_rate("L-93CEC191", "ListSubscriptionsByTopic", 30.0),
        sns_transaction_rate("L-9A72FF33", "ListTagsForResource", 100.0),
        sns_transaction_rate("L-9D5EC8F7", "ListTopics", 30.0),
        // The shape of ONE message, which AWS publishes as fixed: the attributes it may
        // carry and the bytes it may hold.
        quota("Message Attributes per Message", 10.0)
            .code("L-A05852B7")
            .fixed()
            .describe("The number of message attributes in a message."),
        quota("Message Size", 262_144.0)
            .code("L-A228E0EB")
            .unit("Bytes")
            .fixed()
            .describe("The size, in bytes, of a published message."),
        sns_transaction_rate("L-A30AD1BE", "OptInPhoneNumber", 30.0),
        quota("Pending Subscriptions per Account", 5000.0)
            .code("L-A4340BCD")
            .describe(
                "The maximum number of subscriptions awaiting confirmation in this account \
                 in the current Region.",
            ),
        sns_transaction_rate("L-AB928142", "Publish", 30_000.0),
        sns_transaction_rate("L-B3509C67", "PublishBatch", 30_000.0),
        sns_transaction_rate("L-B6107771", "PutDataProtectionPolicy", 30.0),
        sns_transaction_rate("L-B96EDA7D", "RemovePermission", 30.0),
        sns_transaction_rate("L-B98ECD3E", "SetEndpointAttributes", 15.0),
        sns_transaction_rate("L-BBD4D2FF", "SetPlatformApplicationAttributes", 15.0),
        sns_transaction_rate("L-BCD4AAF3", "SetSMSAttributes", 30.0),
        sns_transaction_rate("L-BF824DE9", "SetSubscriptionAttributes", 30.0),
        sns_transaction_rate("L-C1E19420", "SetTopicAttributes", 30.0),
        sns_transaction_rate("L-CC5551AD", "Subscribe", 100.0),
        quota("Subscriptions per Topic", 12_500_000.0)
            .code("L-E1E48E53")
            .describe("The maximum number of subscriptions on one topic."),
        sns_transaction_rate("L-F35E8445", "TagResource", 100.0),
        quota("Topics per Account", 100_000.0)
            .code("L-F514C636")
            .describe("The maximum number of topics in this account in the current Region."),
        sns_transaction_rate("L-F6A3071A", "Unsubscribe", 1000.0),
        sns_transaction_rate("L-F8E2BA85", "UntagResource", 100.0),
        sns_transaction_rate("L-FF07E5EA", "VerifySMSSandboxPhoneNumber", 30.0),
    ]
}
