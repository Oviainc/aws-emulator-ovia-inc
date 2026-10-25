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
//! content belongs to the pillar, not to the request that asked for it. The DETAIL members
//! a single read adds to the summary — what was detected, what the impact is, what the
//! remediation is and what acting on it costs elsewhere — belong to the same finding, so
//! they are rows of the same table rather than a second source of words.
//!
//! Every entry's text sits inside the bounds the modelled shapes publish
//! (`AgentRecommendationSummary$title` `@length(3, 120)`, `$description`
//! `@length(80, 500)`, `Roi$estimate` `@length(2, 40)`, `Roi$detail` `@length(30, 300)`,
//! `Highlight` `@length(20, 250)`, `ImpactDetail` `@length(10, 100)`,
//! `Insight$usagePattern` `@length(100, 2000)`, `$signalsDetected` `@length(30, 1500)`,
//! `RemediationSummary$recommendation` `@length(80, 240)`, `RecommendedFixStep`
//! `@length(20, 200)`, `RemediationStep$content` `@length(80, 8000)` —
//! `smithy_query(query="shape", shape="AgentRecommendationSummary")` / `shape="Roi"` /
//! `shape="Insight"` / `shape="RemediationSummary"` / `shape="RemediationStep"`), the lists
//! hold as many entries as their own bounds allow (`ImpactDetails` `@length(2, 3)`,
//! `Highlights` and `InsightList` `@length(1, 5)`, `RecommendedFixSteps` `@length(1, 5)`,
//! `RemediationSteps` `@length(1, 10)`) and its enum-typed members carry values those enums
//! publish (`Priority`, `Effort`, `ImpactCategory`, `Pillar`, `RiskRating`,
//! `RemediationType`). A test pins all three, and pins that the table covers every pillar
//! the `Pillar` enum publishes — a pillar with no entry here is a pillar a generation can
//! analyse and this emulator holds no finding for, so it would produce no recommendation.

use std::time::SystemTime;

use crate::ops::types::{
    AgentRecommendationRemediation, CrossPillarBenefit, Insight, RemediationStep,
    RemediationSummary, Roi, TradeOff,
};

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
    /// `$highlights`: "highlights describing what was detected" — the observations the
    /// analysis made about this pillar.
    pub highlights: &'static [&'static str],
    /// `$impactDetails`: "detailed impact information for the recommendation" — what
    /// leaving the finding in place costs, in short phrases.
    pub impact_details: &'static [&'static str],
    /// `$insights`: "a list of insights about the recommendation" — the usage pattern the
    /// analysis read and the signals it read it from.
    pub insights: &'static [InsightContent],
    /// `RemediationSummary$recommendation`: "a high-level summary of the recommended
    /// remediation", in one sentence.
    pub remediation_recommendation: &'static str,
    /// `RemediationSummary$steps`: the recommended fix, as the steps it takes.
    pub remediation_steps: &'static [&'static str],
    /// `$crossPillarBenefits`: what acting on this finding also buys under ANOTHER pillar.
    pub cross_pillar_benefits: &'static [CrossPillarBenefitContent],
    /// `$tradeOffs`: what acting on this finding costs under another pillar, and how that
    /// cost is mitigated.
    pub trade_offs: &'static [TradeOffContent],
    /// `$remediations`: "a list of remediations for the recommendation" — the same fix
    /// expressed for each way of applying it, which is what a request's `remediationType`
    /// selects among.
    pub remediations: &'static [RemediationContent],
}

/// One published insight of a finding: the shape of `Insight`, as the table holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InsightContent {
    /// `Insight$usagePattern`: the pattern the analysis read in the workload.
    pub usage_pattern: &'static str,
    /// `Insight$signalsDetected`: the signals that pattern was read from. Optional on the
    /// shape, and every entry here carries one.
    pub signals_detected: &'static str,
}

/// One published cross-pillar benefit of a finding: the shape of `CrossPillarBenefit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrossPillarBenefitContent {
    /// The OTHER `Pillar` the benefit lands under.
    pub pillar: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// An `ImpactCategory`: how large the benefit under that pillar is.
    pub impact: &'static str,
}

/// One published trade-off of a finding: the shape of `TradeOff`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradeOffContent {
    /// The `Pillar` the cost lands under.
    pub pillar: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// A `RiskRating`: how much the trade-off risks.
    pub risk: &'static str,
    pub mitigation: &'static str,
    pub risk_explanation: &'static str,
}

/// One published remediation of a finding: the same fix expressed for one
/// `RemediationType`, as `AgentRecommendationRemediation` carries it. The ARN and the
/// identity and instant the remediation is attributed to belong to the RECOMMENDATION, so
/// they are not held here — see [`RecommendationContent::remediation_list`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemediationContent {
    /// The `RemediationType` this remediation applies the fix through.
    pub remediation_type: &'static str,
    pub steps: &'static [RemediationStepContent],
}

/// One step of a published remediation: the shape of `RemediationStep`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemediationStepContent {
    /// `RemediationStep$title`. Optional on the shape, and every entry here carries one.
    pub title: &'static str,
    pub content: &'static str,
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

    /// `$highlights` as the wire carries it.
    pub fn highlight_list(&self) -> Vec<String> {
        self.highlights
            .iter()
            .map(|highlight| (*highlight).to_string())
            .collect()
    }

    /// `$impactDetails` as the wire carries it.
    pub fn impact_detail_list(&self) -> Vec<String> {
        self.impact_details
            .iter()
            .map(|detail| (*detail).to_string())
            .collect()
    }

    /// `$insights` as the wire carries them.
    pub fn insight_list(&self) -> Vec<Insight> {
        self.insights
            .iter()
            .map(|insight| Insight {
                usage_pattern: insight.usage_pattern.to_string(),
                signals_detected: Some(insight.signals_detected.to_string()),
            })
            .collect()
    }

    /// `$remediationSummary` as the wire carries it: the one-sentence recommendation and
    /// the steps it takes.
    pub fn remediation_summary(&self) -> RemediationSummary {
        RemediationSummary {
            recommendation: self.remediation_recommendation.to_string(),
            steps: self
                .remediation_steps
                .iter()
                .map(|step| (*step).to_string())
                .collect(),
        }
    }

    /// `$crossPillarBenefits` as the wire carries them, or `None` when the finding
    /// publishes none: the member is modelled optional, so a finding with nothing to say
    /// about another pillar says nothing.
    pub fn cross_pillar_benefit_list(&self) -> Option<Vec<CrossPillarBenefit>> {
        (!self.cross_pillar_benefits.is_empty()).then(|| {
            self.cross_pillar_benefits
                .iter()
                .map(|benefit| CrossPillarBenefit {
                    pillar: benefit.pillar.to_string(),
                    title: benefit.title.to_string(),
                    description: benefit.description.to_string(),
                    impact: benefit.impact.to_string(),
                })
                .collect()
        })
    }

    /// `$tradeOffs` as the wire carries them, or `None` when the finding publishes none,
    /// for the reason [`Self::cross_pillar_benefit_list`] gives.
    pub fn trade_off_list(&self) -> Option<Vec<TradeOff>> {
        (!self.trade_offs.is_empty()).then(|| {
            self.trade_offs
                .iter()
                .map(|trade_off| TradeOff {
                    pillar: trade_off.pillar.to_string(),
                    title: trade_off.title.to_string(),
                    description: trade_off.description.to_string(),
                    risk: trade_off.risk.to_string(),
                    mitigation: trade_off.mitigation.to_string(),
                    risk_explanation: Some(trade_off.risk_explanation.to_string()),
                })
                .collect()
        })
    }

    /// `$remediations` as the wire carries them, optionally narrowed to ONE
    /// `RemediationType`.
    ///
    /// A remediation is this finding's fix expressed for one way of applying it, so its
    /// steps come from the table here while the ARN it names and the identity and instant
    /// it is attributed to are the RECOMMENDATION's, passed in: the service published the
    /// two together, so a remediation carries no identity of its own.
    ///
    /// `remediation_type` is a request's filter — "optional filter on remediation type"
    /// (`GetAgentRecommendationRequest$remediationType`) — so a request that names one
    /// keeps the remediations of exactly that type, and a request that names none keeps
    /// every one of them. Reporting the selection rather than `None` when it is empty is
    /// what distinguishes "no remediation of that type" from a finding with no remediation
    /// at all; the member carries no minimum length. `resourceLinks` is left absent: it is
    /// modelled optional (`@length(min: 0)`) and this emulator publishes no links beside a
    /// step.
    pub fn remediation_list(
        &self,
        recommendation_arn: &str,
        published_by: &str,
        published_at: SystemTime,
        remediation_type: Option<&str>,
    ) -> Vec<AgentRecommendationRemediation> {
        self.remediations
            .iter()
            .filter(|remediation| {
                remediation_type.is_none_or(|wanted| wanted == remediation.remediation_type)
            })
            .map(|remediation| AgentRecommendationRemediation {
                recommendation_arn: recommendation_arn.to_string(),
                r#type: remediation.remediation_type.to_string(),
                steps: remediation
                    .steps
                    .iter()
                    .map(|step| RemediationStep {
                        title: Some(step.title.to_string()),
                        content: step.content.to_string(),
                    })
                    .collect(),
                resource_links: None,
                created_by: published_by.to_string(),
                created_at: ::aws_smithy_types::DateTime::from(published_at),
                last_modified_by: None,
                last_modified_at: None,
            })
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
        highlights: &[
            "Average CPU utilization across the analysed instance fleet stays below a fifth \
             of the capacity provisioned for it.",
            "Unattached block storage volumes and idle load balancers remain billed in every \
             analysed Region.",
            "No compute commitment covers the steady baseline the workload has sustained for \
             the whole analysed period.",
        ],
        impact_details: &[
            "On-demand spend on capacity the workload never reaches",
            "Surplus capacity grows with every new environment",
        ],
        insights: &[InsightContent {
            usage_pattern: "Demand follows a weekday business cycle with a narrow peak each \
                            afternoon, and the fleet is sized for that peak around the clock, \
                            so most of the provisioned capacity is idle outside a few hours a \
                            day.",
            signals_detected: "Sustained low CPU and memory utilization, unattached volumes, \
                               and no active compute commitment.",
        }],
        remediation_recommendation: "Right-size the fleet to the utilization the workload \
                                     sustains, remove the storage and load balancers nothing \
                                     uses, then cover the remaining baseline with a compute \
                                     commitment.",
        remediation_steps: &[
            "Right-size the instances whose sustained utilization is below the fleet's target.",
            "Delete the unattached volumes and the load balancers with no registered targets.",
            "Cover the baseline that remains with a Savings Plan or Reserved Instance \
             commitment.",
        ],
        cross_pillar_benefits: &[CrossPillarBenefitContent {
            pillar: "OPERATIONAL_EXCELLENCE",
            title: "Fewer resources to operate",
            description: "Removing the capacity nothing uses leaves fewer instances, volumes \
                          and load balancers for an operator to patch, monitor and reason \
                          about.",
            impact: "MEDIUM",
        }],
        trade_offs: &[TradeOffContent {
            pillar: "RESILIENCE",
            title: "Less headroom for an unplanned spike",
            description: "Capacity sized to sustained utilization absorbs less of a sudden \
                          spike, so a demand burst the oversized fleet would have swallowed \
                          now has to be met by scaling out.",
            risk: "MEDIUM",
            mitigation: "Keep an automatic scaling policy with a buffer over the measured \
                         peak, so the fleet grows before a spike reaches the capacity that \
                         remains.",
            risk_explanation: "The spare capacity that hid demand bursts is what the \
                               right-sizing removes.",
        }],
        remediations: &[
            RemediationContent {
                remediation_type: "CONSOLE",
                steps: &[
                    RemediationStepContent {
                        title: "Review the right-sizing findings",
                        content: "Open the compute right-sizing findings for the analysed \
                                  accounts, sort them by projected monthly saving, and apply \
                                  the instance type each finding proposes to the fleet it \
                                  names.",
                    },
                    RemediationStepContent {
                        title: "Release the idle storage",
                        content: "List the block storage volumes in the available state and \
                                  the load balancers with no registered targets, snapshot \
                                  anything that still has to be kept, and delete the rest.",
                    },
                ],
            },
            RemediationContent {
                remediation_type: "CLI",
                steps: &[
                    RemediationStepContent {
                        title: "Resize from a script",
                        content: "Stop each instance the findings name, change its instance \
                                  type to the proposed one, then start it again and confirm \
                                  the workload's health checks pass before moving on.",
                    },
                    RemediationStepContent {
                        title: "Buy the commitment",
                        content: "Describe the sustained hourly usage of the fleet that \
                                  remains, then purchase a commitment covering that baseline \
                                  for the term the workload's finance owner approved.",
                    },
                ],
            },
        ],
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
        highlights: &[
            "Long-lived access keys are still issued to everyday principals in every analysed \
             account.",
            "Administrative policies are attached directly to users rather than to roles \
             assumed on demand.",
            "Several access keys in the analysed accounts have not been rotated for more than \
             a year.",
        ],
        impact_details: &[
            "One leaked key grants standing access to the workload",
            "The blast radius spans the data and the control plane",
        ],
        insights: &[InsightContent {
            usage_pattern: "Day-to-day work is done under principals that hold their own \
                            long-lived keys, and those principals carry administrative \
                            permissions continuously rather than assuming a role for the task \
                            that needs them.",
            signals_detected: "Long-lived access keys, administrative policies attached \
                               directly to users, and keys past their rotation age.",
        }],
        remediation_recommendation: "Replace the long-lived access keys with short-lived \
                                     credentials from an assumed role, and scope every \
                                     administrative permission to the task that needs it.",
        remediation_steps: &[
            "Inventory the access keys and the principals holding administrative policies.",
            "Move each workflow onto a role it assumes for the task, with scoped permissions.",
            "Deactivate, then delete, the access keys nothing uses any more.",
        ],
        cross_pillar_benefits: &[CrossPillarBenefitContent {
            pillar: "OPERATIONAL_EXCELLENCE",
            title: "One place to grant and revoke access",
            description: "Access granted through a role is revoked by changing that role, \
                          rather than by hunting for the keys a principal has copied \
                          elsewhere.",
            impact: "MEDIUM",
        }],
        trade_offs: &[TradeOffContent {
            pillar: "OPERATIONAL_EXCELLENCE",
            title: "An extra step before privileged work",
            description: "An operator who used to act directly now assumes a role first, so \
                          an urgent change carries one more step and depends on the identity \
                          provider being reachable.",
            risk: "LOW",
            mitigation: "Keep a break-glass role with its own audited credentials, so \
                         privileged work is still possible while the identity provider is \
                         unavailable.",
            risk_explanation: "The added step is a sign-in, not a redesign of the workload.",
        }],
        remediations: &[
            RemediationContent {
                remediation_type: "CONSOLE",
                steps: &[
                    RemediationStepContent {
                        title: "Replace the keys",
                        content: "For each principal holding a long-lived key, create the role \
                                  its workflow should assume, grant the scoped permissions to \
                                  that role, and deactivate the key once the workflow uses it.",
                    },
                    RemediationStepContent {
                        title: "Detach the standing administrative policies",
                        content: "Detach the administrative policies attached directly to \
                                  users, attach them to a role instead, and let the people who \
                                  need them assume that role for the change they are making.",
                    },
                ],
            },
            RemediationContent {
                remediation_type: "IAC",
                steps: &[
                    RemediationStepContent {
                        title: "Describe the roles in a template",
                        content: "Declare each role, its trust policy and its scoped \
                                  permission policy in the stack that owns the account's \
                                  identity resources, so a grant is reviewed like any other \
                                  change.",
                    },
                    RemediationStepContent {
                        title: "Fail the pipeline on a new key",
                        content: "Add a policy check to the deployment pipeline that refuses a \
                                  template declaring a long-lived access key, so the accounts \
                                  cannot drift back to the pattern just removed.",
                    },
                ],
            },
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
        highlights: &[
            "The primary data store runs in a single Availability Zone with no standby \
             alongside it.",
            "No recovery of the workload from its backups has been rehearsed in the analysed \
             period.",
            "Several components have no capacity outside the one zone that hosts them today.",
        ],
        impact_details: &[
            "Losing one zone takes the whole workload down",
            "Recovery time is bound by a restore from backup",
        ],
        insights: &[InsightContent {
            usage_pattern: "Every tier of the workload is deployed into one Availability Zone, \
                            including the database it reads on the critical path, and the \
                            recovery procedure on file has never been exercised against a real \
                            restore.",
            signals_detected: "A single-zone database, no standby alongside it, and no \
                               rehearsed restore in the analysed period.",
        }],
        remediation_recommendation: "Give every tier capacity in a second Availability Zone, \
                                     put the primary data store behind a standby in that zone, \
                                     and rehearse the recovery on a schedule.",
        remediation_steps: &[
            "Enable a standby for the primary data store in a second Availability Zone.",
            "Spread the compute tiers and their load balancer across both zones.",
            "Rehearse a failover and a restore, and record how long each one took.",
        ],
        cross_pillar_benefits: &[CrossPillarBenefitContent {
            pillar: "PERFORMANCE",
            title: "Read capacity in a second zone",
            description: "A standby that can serve reads adds capacity read-heavy traffic can \
                          be directed to, which leaves the primary with the writes.",
            impact: "MEDIUM",
        }],
        trade_offs: &[TradeOffContent {
            pillar: "COST_OPTIMIZATION",
            title: "Standby capacity is paid for",
            description: "Capacity in a second zone and a standby database are billed whether \
                          or not the zone they protect against ever fails, so the monthly cost \
                          of the workload rises.",
            risk: "MEDIUM",
            mitigation: "Size the second zone for the traffic a failover has to carry rather \
                         than as a copy of the whole fleet, and cover it with the same \
                         commitment as the baseline.",
            risk_explanation: "The cost is continuous; the outage it prevents is not.",
        }],
        remediations: &[
            RemediationContent {
                remediation_type: "IAC",
                steps: &[
                    RemediationStepContent {
                        title: "Declare the second zone",
                        content: "Add the second Availability Zone's subnets, the standby \
                                  database instance and the load balancer targets to the stack \
                                  that owns the workload, and deploy it to the analysed \
                                  accounts.",
                    },
                    RemediationStepContent {
                        title: "Pin the recovery objectives",
                        content: "Record the recovery time and recovery point the workload is \
                                  designed for in the same template, so a later change that \
                                  would break either of them is visible in review.",
                    },
                ],
            },
            RemediationContent {
                remediation_type: "AUTO_REMEDIATION",
                steps: &[
                    RemediationStepContent {
                        title: "Let the service fail over",
                        content: "Enable the managed multi-zone deployment of the data store, \
                                  so the service promotes the standby itself when the zone \
                                  hosting the primary becomes unavailable.",
                    },
                    RemediationStepContent {
                        title: "Schedule the rehearsal",
                        content: "Run the failover rehearsal on a schedule and record its \
                                  outcome, so the recovery path is exercised by the automation \
                                  rather than by the next real incident.",
                    },
                ],
            },
        ],
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
        highlights: &[
            "At peak the same read queries are repeated against the primary data store.",
            "Response time tracks database load rather than the size of the result set.",
            "No cache sits between the application tier and the data it reads most often.",
        ],
        impact_details: &[
            "Tail latency at peak is bound by database capacity",
            "A capacity increase buys latency, not throughput",
        ],
        insights: &[InsightContent {
            usage_pattern: "A small set of read queries accounts for most of the traffic at \
                            peak and returns results that change far less often than they are \
                            read, so the primary data store answers the same question many \
                            times a minute.",
            signals_detected: "Repeated identical reads at peak, latency rising with database \
                               load, and no cache tier in front of the store.",
        }],
        remediation_recommendation: "Put a cache in front of the primary data store for the \
                                     reads that repeat, and let the application fall back to \
                                     the store whenever the cache holds no entry.",
        remediation_steps: &[
            "Identify the read queries that repeat most often at peak.",
            "Serve those reads from a cache with a time to live the data tolerates.",
            "Measure tail latency before and after, and keep the fallback path tested.",
        ],
        cross_pillar_benefits: &[CrossPillarBenefitContent {
            pillar: "COST_OPTIMIZATION",
            title: "A deferred database upgrade",
            description: "Reads the cache absorbs are reads the primary never sees, so the \
                          next database capacity increase waits until write volume needs it.",
            impact: "MEDIUM",
        }],
        trade_offs: &[TradeOffContent {
            pillar: "RESILIENCE",
            title: "A cache is one more thing to lose",
            description: "The workload now depends on a cache tier as well as on its data \
                          store, and a cache that empties sends the whole read load back to \
                          the primary at once.",
            risk: "MEDIUM",
            mitigation: "Keep the fallback to the data store on every read path and limit how \
                         much load a cold cache can pass through, so an empty cache degrades \
                         latency instead of the workload.",
            risk_explanation: "A cold cache is a latency event only while it refills.",
        }],
        remediations: &[
            RemediationContent {
                remediation_type: "CONSOLE",
                steps: &[
                    RemediationStepContent {
                        title: "Create the cache",
                        content: "Create a cache cluster in the subnets the application tier \
                                  runs in, size it for the working set the repeated reads \
                                  touch, and allow only that tier to reach it.",
                    },
                    RemediationStepContent {
                        title: "Watch the hit rate",
                        content: "Chart the cache hit rate and the database read latency side \
                                  by side, so the time to live can be tuned against what the \
                                  workload actually reads.",
                    },
                ],
            },
            RemediationContent {
                remediation_type: "SDK",
                steps: &[
                    RemediationStepContent {
                        title: "Read through the cache",
                        content: "Wrap the repeated reads so each one looks in the cache \
                                  first, falls back to the data store on a miss, and writes \
                                  what it read back with the agreed time to live.",
                    },
                    RemediationStepContent {
                        title: "Invalidate on write",
                        content: "Remove the cached entry whenever the write path changes the \
                                  rows behind it, so a reader never sees a value the data \
                                  store has already replaced.",
                    },
                ],
            },
        ],
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
        highlights: &[
            "Changes and recoveries still depend on steps an operator performs by hand.",
            "The outcome of a change varies with whoever happens to be on call for it.",
            "No evidence exists that the recovery procedure works until an incident tests it.",
        ],
        impact_details: &[
            "Recovery depends on who is on call for it",
            "A failed manual change costs rework",
        ],
        insights: &[InsightContent {
            usage_pattern: "Routine changes and recoveries are carried out from written notes \
                            rather than from automation, so each run depends on the operator \
                            reading them, and the procedures are exercised only once something \
                            has already gone wrong.",
            signals_detected: "Manual change steps, no codified runbook, and no rehearsal \
                               recorded in the analysed period.",
        }],
        remediation_recommendation: "Codify the manual change and recovery steps as runbooks \
                                     the automation runs, and rehearse them on a schedule so \
                                     the procedure is known to work before it is needed.",
        remediation_steps: &[
            "Write down the change and recovery steps performed by hand today.",
            "Turn each of them into a runbook the automation can run end to end.",
            "Rehearse the runbooks on a schedule and record what each run took.",
        ],
        cross_pillar_benefits: &[CrossPillarBenefitContent {
            pillar: "RESILIENCE",
            title: "A recovery that is known to work",
            description: "A runbook rehearsed on a schedule turns the recovery procedure from \
                          a document into a path the workload has actually taken.",
            impact: "HIGH",
        }],
        trade_offs: &[TradeOffContent {
            pillar: "COST_OPTIMIZATION",
            title: "Codifying the steps takes time",
            description: "Writing and rehearsing runbooks spends operator time that would \
                          otherwise go to feature work, and the automation itself has to be \
                          maintained as the workload changes.",
            risk: "LOW",
            mitigation: "Codify the procedures that are run most often first, so the time \
                         spent is repaid by the runs it saves rather than by the ones that \
                         never happen.",
            risk_explanation: "The cost is operator time, spent once per procedure.",
        }],
        remediations: &[
            RemediationContent {
                remediation_type: "CLI",
                steps: &[
                    RemediationStepContent {
                        title: "Capture the steps",
                        content: "Record each manual change as a sequence of commands with its \
                                  checks, commit it beside the workload's own code, and run it \
                                  from there rather than from an operator's notes.",
                    },
                    RemediationStepContent {
                        title: "Rehearse from the command line",
                        content: "Run the captured sequence against a non-production copy of \
                                  the workload, fix whatever the run reveals, and record how \
                                  long the rehearsal took.",
                    },
                ],
            },
            RemediationContent {
                remediation_type: "MCP",
                steps: &[
                    RemediationStepContent {
                        title: "Expose the runbooks to an assistant",
                        content: "Publish the codified runbooks through a Model Context \
                                  Protocol server, so an operator who asks for a procedure is \
                                  given the same steps the automation would run.",
                    },
                    RemediationStepContent {
                        title: "Keep the published set current",
                        content: "Regenerate the published runbook set whenever a procedure \
                                  changes, so the steps an assistant reports are the steps the \
                                  automation would run today.",
                    },
                ],
            },
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
