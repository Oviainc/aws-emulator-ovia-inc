// SPDX-License-Identifier: CC0-1.0

//! AWS WAF's published default-quota catalog, under the `wafv2` `ServiceCode`.

use super::*;

/// AWS WAF's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `wafv2` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code wafv2` reports, which is
/// the "AWS WAF quotas" page of the AWS WAF Developer Guide), and the `QuotaCode`s that
/// catalog is keyed on.
///
/// Two rows are published as quotas OF A RESOURCE rather than of the account: the web ACL
/// capacity units of one web ACL and of one rule group may be raised for an individual
/// resource as well as account-wide, so each names the level AWS publishes it as
/// adjustable at and the resource type that plane applies to
/// ([`QuotaRow::adjustable_at`]), and reports the RESOURCE scope on every record of it,
/// because the plane it is counted on is the resource whichever record is read
/// ([`QuotaRow::counted_per_resource`]). The row published as a RATE carries the
/// one-second window AWS publishes it over and the `AWS/Usage` `CallCount` of the API it
/// throttles.
pub(in crate::state) fn wafv2_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Custom response bodies per web ACL", 50.0)
            .code("L-0224FEE0")
            .fixed()
            .describe("The maximum number of custom response bodies in one web ACL."),
        quota("Custom headers per rule", 5.0)
            .code("L-030B54CC")
            .fixed()
            .describe("The maximum number of custom headers inserted by one rule."),
        quota("Text transformations per rule", 10.0)
            .code("L-041DD6D3")
            .fixed()
            .describe("The maximum number of text transformations in one rule."),
        quota("IP sets per Region", 100.0)
            .code("L-08788A4E")
            .describe("The maximum number of IP sets in this account in the current Region."),
        quota("IP addresses per IP set", 10_000.0)
            .code("L-0A8A309C")
            .describe("The maximum number of IP address ranges in one IP set."),
        quota("Regex pattern sets per Region", 10.0)
            .code("L-11D00F38")
            .describe(
                "The maximum number of regex pattern sets in this account in the current \
                 Region.",
            ),
        quota("Regular expressions per regex pattern set", 10.0)
            .code("L-291AF103")
            .fixed()
            .describe("The maximum number of regular expressions in one regex pattern set."),
        quota("Rule groups per Region", 100.0)
            .code("L-2B217866")
            .describe(
                "The maximum number of rule groups in this account in the current Region.",
            ),
        quota("Rule groups per web ACL", 100.0)
            .code("L-2D9CB303")
            .fixed()
            .describe("The maximum number of rule groups referenced by one web ACL."),
        quota("Web ACLs per Region", 100.0)
            .code("L-2EC3DE7B")
            .describe("The maximum number of web ACLs in this account in the current Region."),
        quota("Rate-based rules per web ACL", 10.0)
            .code("L-3D6A3ADF")
            .fixed()
            .describe("The maximum number of rate-based rules in one web ACL."),
        quota("Logging configurations per web ACL", 1.0)
            .code("L-45C90A8A")
            .fixed()
            .describe("The maximum number of logging configurations on one web ACL."),
        // The capacity of one web ACL, which AWS publishes as adjustable for an
        // individual web ACL as well as for the account, counted on the web ACL.
        quota("Web ACL capacity units (WCU) per web ACL", 1500.0)
            .code("L-5B2F9B11")
            .adjustable_at(LEVEL_ALL, "AWS::WAFv2::WebACL")
            .counted_per_resource()
            .describe("The maximum number of web ACL capacity units used by one web ACL."),
        quota("Managed rule groups per web ACL", 10.0)
            .code("L-5E8DF1EF")
            .fixed()
            .describe("The maximum number of managed rule groups referenced by one web ACL."),
        quota("Labels per rule", 50.0)
            .code("L-6F32B880")
            .fixed()
            .describe("The maximum number of labels added by one rule."),
        quota("Statement nesting depth per rule", 10.0)
            .code("L-71C2E81B")
            .fixed()
            .describe("The maximum nesting depth of the statements of one rule."),
        quota("Token domains per web ACL", 10.0)
            .code("L-74EBD1DD")
            .fixed()
            .describe("The maximum number of token domains configured on one web ACL."),
        quota("Body inspection size for CloudFront distributions", 65_536.0)
            .code("L-8B48556D")
            .unit("Bytes")
            .describe(
                "The maximum size, in bytes, of the request body AWS WAF inspects for a \
                 CloudFront distribution.",
            ),
        quota("Body inspection size for regional resources", 8192.0)
            .code("L-91DEFBB6")
            .unit("Bytes")
            .describe(
                "The maximum size, in bytes, of the request body AWS WAF inspects for a \
                 regional resource.",
            ),
        quota("Web ACLs per Firewall Manager policy", 1.0)
            .code("L-9E6FF091")
            .fixed()
            .describe("The maximum number of web ACLs created by one Firewall Manager policy."),
        quota("Resources per web ACL", 1000.0)
            .code("L-9FD4FE94")
            .describe("The maximum number of resources associated with one web ACL."),
        quota("CAPTCHA immunity time", 300.0)
            .code("L-A85235F6")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a client is immune from CAPTCHA after solving \
                 one.",
            ),
        quota("Challenge immunity time", 300.0)
            .code("L-B1635397")
            .unit("Seconds")
            .fixed()
            .describe(
                "The time, in seconds, that a client is immune from a challenge after \
                 passing one.",
            ),
        quota("Characters per regular expression", 512.0)
            .code("L-B24834C1")
            .fixed()
            .describe("The maximum number of characters in one regular expression."),
        // The capacity of one rule group, published like the web ACL capacity above: an
        // increase may be asked for an individual rule group as well as account-wide.
        quota("Web ACL capacity units (WCU) per rule group", 1500.0)
            .code("L-B32E8672")
            .adjustable_at(LEVEL_ALL, "AWS::WAFv2::RuleGroup")
            .counted_per_resource()
            .describe("The maximum number of web ACL capacity units used by one rule group."),
        quota("IP set references per web ACL", 50.0)
            .code("L-BF0029B0")
            .fixed()
            .describe("The maximum number of IP set references in one web ACL."),
        quota("Scope-down statements per managed rule group", 1.0)
            .code("L-C5BCD850")
            .fixed()
            .describe(
                "The maximum number of scope-down statements on one managed rule group \
                 reference.",
            ),
        quota("Tags per resource", 50.0)
            .code("L-C5E4D334")
            .fixed()
            .describe("The maximum number of tags on one AWS WAF resource."),
        quota("Excluded rules per managed rule group", 50.0)
            .code("L-C928281E")
            .fixed()
            .describe(
                "The maximum number of rules excluded from one managed rule group \
                 reference.",
            ),
        quota("Request sampling window", 3.0)
            .code("L-CCCD1D7B")
            .unit("Hours")
            .fixed()
            .describe("The time, in hours, over which sampled requests are available."),
        request_rate("L-D6DA96AE", WAF_USAGE_SERVICE, "GetSampledRequests", 5.0),
        quota("Versions retained per managed rule group", 3.0)
            .code("L-D95DED7E")
            .fixed()
            .describe(
                "The number of static versions of a managed rule group that remain \
                 available.",
            ),
        quota("Rules per rule group", 100.0)
            .code("L-D9F31E8A")
            .fixed()
            .describe("The maximum number of rules in one rule group."),
        quota("Web ACLs per CloudFront distribution", 1.0)
            .code("L-E4E414A8")
            .fixed()
            .describe("The maximum number of web ACLs associated with one CloudFront distribution."),
    ]
}

/// The name `AWS/Usage` publishes AWS WAF's API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const WAF_USAGE_SERVICE: &str = "WAFV2";
