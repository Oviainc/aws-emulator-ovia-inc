// SPDX-License-Identifier: CC0-1.0

//! The optimization-recommendation content this emulator publishes, and the one lookup
//! every operation that reports a recommendation goes through.
//!
//! A recommendation generation "analyzes your Amazon Web Services resources and generates
//! optimization recommendations based on the configured pillars and scope"
//! (`smithy_query(query="operation", operation="StartAgentRecommendationGeneration")`), so
//! what a completed generation produces is one recommendation per pillar its scope named,
//! carrying that pillar's finding. The pillar is what the request chooses; the CONTENT of
//! the finding — its title, its description, how it is prioritised, what it costs to
//! implement and what the return on that work is — is published by neither the Smithy
//! model nor the service docs in a machine-readable form, so it is this emulator's own,
//! exactly as the lens question sets in [`super::official_lenses`] are.
//!
//! One table, so the three readings of "what does this recommendation say?" — the summary a
//! listing reports, the record a status update resolves and the detail a single read
//! answers — cannot disagree, and so a recommendation's text is stable across reads: the
//! content belongs to the pillar, not to the request that asked for it.
//!
//! Every entry's text sits inside the bounds the modelled shapes publish
//! (`AgentRecommendationSummary$title` `@length(3, 120)`, `$description`
//! `@length(80, 500)`, `Roi$estimate` `@length(2, 40)`, `Roi$detail` `@length(30, 300)` —
//! `smithy_query(query="shape", shape="AgentRecommendationSummary")` / `shape="Roi"`), and
//! its enum-typed members carry values those enums publish (`Priority`, `Effort`,
//! `ImpactCategory`). A test pins both, and pins that the table covers every pillar the
//! `Pillar` enum publishes — a pillar with no entry here is a pillar a generation can
//! analyse and this emulator holds no finding for, so it would produce no recommendation.

use crate::ops::types::Roi;

/// What this emulator publishes as the finding for one pillar: the members of a
/// recommendation that describe the FINDING rather than the generation that produced it or
/// the status a caller has since moved it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecommendationContent {
    /// The `Pillar` this finding is about — the key of the table, and what
    /// `AgentRecommendationSummary$pillar` reports.
    pub pillar: &'static str,
    /// `$title`: what the recommendation asks for, in one line.
    pub title: &'static str,
    /// `$description`: what the analysis found and why it matters.
    pub description: &'static str,
    /// `$priority` — a `Priority` value: how urgent acting on the finding is.
    pub priority: &'static str,
    /// `$effort` — an `Effort` value: how much work implementing it is.
    pub effort: &'static str,
    /// `$impact` — an `ImpactCategory` value: "the severity of the recommendation's
    /// impact".
    pub impact: &'static str,
    /// `Roi$estimate`: the return on the work, as a short phrase. Optional on the shape,
    /// and every entry here carries one.
    pub roi_estimate: &'static str,
    /// `Roi$detail`: what that return consists of. `@required` on the shape.
    pub roi_detail: &'static str,
    /// `$awsServices`: "the Amazon Web Services services that the recommendation applies
    /// to" — the services the finding is about, which is part of the finding rather than a
    /// reading of a customer's resources.
    pub aws_services: &'static [&'static str],
}

impl RecommendationContent {
    /// The `Roi` this finding reports, as the wire carries it.
    pub fn roi(&self) -> Roi {
        Roi {
            estimate: Some(self.roi_estimate.to_string()),
            detail: self.roi_detail.to_string(),
        }
    }

    /// `$awsServices` as the wire carries it: the list is modelled optional, and every
    /// entry here names at least one service.
    pub fn aws_service_list(&self) -> Vec<String> {
        self.aws_services
            .iter()
            .map(|service| (*service).to_string())
            .collect()
    }
}

/// The finding this emulator publishes for each pillar the `Pillar` enum names
/// (`smithy_query(query="shape", shape="Pillar")`), in that enum's order.
///
/// Extensible by construction: a pillar the enum adds is a row here, and nothing else has
/// to change for a generation that scopes it to produce its recommendation.
pub const RECOMMENDATION_CATALOG: &[RecommendationContent] = &[
    RecommendationContent {
        pillar: "COST_OPTIMIZATION",
        title: "Right-size and commit to the capacity the workload actually uses",
        description: "The analysed accounts and Regions provision compute and storage well \
                      above the utilization the workload sustains, so capacity is paid for \
                      at on-demand rates for throughput it never reaches, and the surplus \
                      grows with every new environment.",
        priority: "HIGH",
        effort: "MEDIUM",
        impact: "HIGH",
        roi_estimate: "High, within one billing cycle",
        roi_detail: "Right-sizing removes capacity nothing uses, and a commitment then \
                     covers the baseline that remains, so the saving recurs every billing \
                     cycle instead of once.",
        aws_services: &["Amazon EC2", "Amazon EBS", "AWS Auto Scaling"],
    },
    RecommendationContent {
        pillar: "SECURITY",
        title: "Remove long-lived credentials and standing administrative access",
        description: "The analysed accounts still issue long-lived access keys and attach \
                      broad administrative policies to everyday principals, so one leaked \
                      credential would give an attacker standing access to the workload's \
                      data and to its control plane.",
        priority: "HIGH",
        effort: "MEDIUM",
        impact: "HIGH",
        roi_estimate: "Risk reduction, not financial",
        roi_detail: "Short-lived credentials and scoped roles remove the standing access an \
                     attacker needs, so the return is a lower expected cost of a credential \
                     compromise rather than a smaller bill.",
        aws_services: &[
            "AWS Identity and Access Management",
            "AWS IAM Identity Center",
            "AWS Secrets Manager",
        ],
    },
    RecommendationContent {
        pillar: "RESILIENCE",
        title: "Make the workload survive the loss of one Availability Zone",
        description: "Parts of the workload — its primary data store among them — run in a \
                      single Availability Zone with no rehearsed recovery path, so the loss \
                      of that zone takes the workload down for as long as rebuilding it \
                      from backups takes.",
        priority: "HIGH",
        effort: "LARGE",
        impact: "HIGH",
        roi_estimate: "Avoided outage cost",
        roi_detail: "Capacity in a second zone and a recovery path somebody has rehearsed \
                     turn the loss of a zone into a degraded period rather than an outage, \
                     which is what the work buys.",
        aws_services: &["Amazon RDS", "Elastic Load Balancing", "AWS Backup"],
    },
    RecommendationContent {
        pillar: "PERFORMANCE",
        title: "Serve the repeated reads from a cache instead of the primary data store",
        description: "At peak the workload repeats the same read queries against its \
                      primary data store, so response time is bound by database capacity \
                      that a cache in front of it would absorb, and the next capacity \
                      increase buys latency rather than throughput.",
        priority: "MEDIUM",
        effort: "SMALL",
        impact: "MEDIUM",
        roi_estimate: "Medium, within a quarter",
        roi_detail: "Caching the repeated reads cuts tail latency at peak and defers the \
                     next database upgrade, so the return shows up both in user-facing \
                     latency and in capacity cost.",
        aws_services: &["Amazon ElastiCache", "Amazon CloudFront", "Amazon RDS"],
    },
    RecommendationContent {
        pillar: "OPERATIONAL_EXCELLENCE",
        title: "Replace the manual change and recovery steps with tested runbooks",
        description: "Changes and recoveries still depend on steps an operator performs by \
                      hand, so the outcome varies with who is on call and the workload has \
                      no evidence that its recovery procedure works until an incident tests \
                      it.",
        priority: "MEDIUM",
        effort: "MEDIUM",
        impact: "MEDIUM",
        roi_estimate: "Medium, operational",
        roi_detail: "Codified, rehearsed procedures shorten recovery and remove the rework a \
                     failed manual change causes, so the return is in operator time and in \
                     avoided incident minutes.",
        aws_services: &[
            "AWS Systems Manager",
            "Amazon CloudWatch",
            "AWS CloudFormation",
        ],
    },
];

/// The finding this emulator publishes for `pillar`, or `None` when it publishes none —
/// which is what makes the table above the only place a pillar's content is decided.
pub fn recommendation_content(pillar: &str) -> Option<&'static RecommendationContent> {
    RECOMMENDATION_CATALOG
        .iter()
        .find(|content| content.pillar == pillar)
}
