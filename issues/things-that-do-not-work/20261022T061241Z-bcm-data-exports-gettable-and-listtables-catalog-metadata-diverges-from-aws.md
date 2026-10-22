Issue-Source: trace-sweep-divergence
Issue-Trace: bcm-data-exports-catalog-get-table
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bcm-data-exports-catalog-get-table bcm-data-exports-catalog-list-tables bcm-data-exports-table-catalog-inspection
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports|GetTable|2xx->4xx|$.Description
Issue-Key: bcm-data-exports|GetTable|2xx->4xx|$.Schema
Issue-Key: bcm-data-exports|GetTable|2xx->4xx|$.TableName
Issue-Key: bcm-data-exports|GetTable|2xx->4xx|status
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_bill_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_bill_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_entity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_entity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_end_date"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_end_date"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_start_date"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_start_date"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoice_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoice_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoicing_entity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoicing_entity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_name"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_name"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="cost_category"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="cost_category"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="discount_bundled_discount"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="discount_total_discount"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_line_item_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_line_item_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_time_interval"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_time_interval"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_availability_zone"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_availability_zone"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_rate"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_rate"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_currency_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_currency_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_legal_entity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_legal_entity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_description"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_description"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_rate"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_rate"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalization_factor"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalization_factor"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalized_usage_amount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalized_usage_amount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_operation"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_operation"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_product_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_product_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_tax_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_tax_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_rate"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_rate"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_name"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_name"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_amount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_amount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_end_date"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_end_date"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_start_date"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_start_date"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_user_identifier"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_currency"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_currency"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_lease_contract_length"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_lease_contract_length"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_offering_class"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_offering_class"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_rate"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_rate"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_purchase_option"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_purchase_option"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_term"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_term"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_unit"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_unit"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_comment"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_comment"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_description"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_description"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_region_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_region_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_family"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_family"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instancesku"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instancesku"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_operation"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_operation"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_pricing_unit"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_pricing_unit"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_product_family"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_product_family"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_region_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_region_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_servicecode"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_servicecode"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_sku"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_sku"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_region_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_region_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_usagetype"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_usagetype"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_cost_for_usage"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_cost_for_usage"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_fee_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_fee_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_availability_zone"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_availability_zone"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_effective_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_effective_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_end_time"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_end_time"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_modification_status"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_modification_status"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_cost_for_usage"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_cost_for_usage"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_fee_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_fee_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_effective_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_effective_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_recurring_fee_for_usage"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_recurring_fee_for_usage"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_amortized_upfront_fee_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_amortized_upfront_fee_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_recurring_fee"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_recurring_fee"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_upfront_value"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_upfront_value"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_normalized_units_per_reservation"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_normalized_units_per_reservation"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_number_of_reservations"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_number_of_reservations"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_recurring_fee_for_usage"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_recurring_fee_for_usage"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_reservation_a_r_n"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_reservation_a_r_n"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_start_time"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_start_time"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_subscription_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_subscription_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_normalized_units"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_normalized_units"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_units"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_units"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_units_per_reservation"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_units_per_reservation"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_amortized_upfront_fee_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_amortized_upfront_fee_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_normalized_unit_quantity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_normalized_unit_quantity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_quantity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_quantity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_recurring_fee"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_recurring_fee"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_upfront_value"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_upfront_value"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="resource_tags"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_amortized_upfront_commitment_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_amortized_upfront_commitment_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_end_time"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_end_time"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_instance_type_family"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_instance_type_family"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_amortized_upfront_commitment_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_amortized_upfront_commitment_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_recurring_commitment_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_recurring_commitment_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_savings_plan_effective_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_savings_plan_effective_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_offering_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_offering_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_payment_option"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_payment_option"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_purchase_term"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_purchase_term"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_recurring_commitment_for_billing_period"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_recurring_commitment_for_billing_period"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_region"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_region"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_a_r_n"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_a_r_n"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_effective_cost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_effective_cost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_rate"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_rate"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_start_time"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_start_time"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_total_commitment_to_date"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_total_commitment_to_date"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_used_commitment"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_used_commitment"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="tags"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="account_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="account_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="account_name"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="action_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="action_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="currency_code"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="currency_code"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_details"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_details"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_summary"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_summary"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="current_resource_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_cost_after_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_cost_after_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_cost_before_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_cost_before_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_savings_after_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_savings_after_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_savings_before_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_monthly_savings_before_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_savings_percentage_after_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_savings_percentage_after_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_savings_percentage_before_discount"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="estimated_savings_percentage_before_discount"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="implementation_effort"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="implementation_effort"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="last_refresh_timestamp"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="last_refresh_timestamp"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommendation_id"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommendation_id"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommendation_lookback_period_in_days"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommendation_source"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommendation_source"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_details"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_details"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_summary"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_summary"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_type"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="recommended_resource_type"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="region"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="region"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="resource_arn"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="resource_arn"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="restart_needed"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="restart_needed"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="rollback_possible"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="rollback_possible"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="tags"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="tags"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.TableProperties.FILTER
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="AvailabilityZone"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="AvailabilityZone"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BilledCost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BilledCost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingAccountId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingAccountId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingAccountName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingAccountName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingCurrency"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingCurrency"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingPeriodEnd"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingPeriodEnd"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingPeriodStart"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="BillingPeriodStart"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeCategory"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeCategory"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeClass"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeClass"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeDescription"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeDescription"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeFrequency"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargeFrequency"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargePeriodEnd"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargePeriodEnd"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargePeriodStart"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ChargePeriodStart"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountCategory"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountCategory"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountStatus"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountStatus"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountType"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="CommitmentDiscountType"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ConsumedQuantity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ConsumedQuantity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ConsumedUnit"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ConsumedUnit"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ContractedCost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ContractedCost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ContractedUnitPrice"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ContractedUnitPrice"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="EffectiveCost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="EffectiveCost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="InvoiceIssuerName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="InvoiceIssuerName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ListCost"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ListCost"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ListUnitPrice"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ListUnitPrice"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingCategory"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingCategory"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingQuantity"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingQuantity"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingUnit"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PricingUnit"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ProviderName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ProviderName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PublisherName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="PublisherName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="RegionId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="RegionId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="RegionName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="RegionName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceType"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ResourceType"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ServiceCategory"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ServiceCategory"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ServiceName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="ServiceName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SkuId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SkuId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SkuPriceId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SkuPriceId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SubAccountId"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SubAccountId"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SubAccountName"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="SubAccountName"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="Tags"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="Tags"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_CostCategories"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_CostCategories"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_Discounts"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_Discounts"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_Operation"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_Operation"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_ServiceCode"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_ServiceCode"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_UsageType"].Description
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="x_UsageType"].Type
Issue-Key: bcm-data-exports|GetTable|2xx->4xx|$.TableProperties
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="CARBON_EMISSIONS"]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_AND_USAGE_DASHBOARD"]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_AND_USAGE_REPORT"].Description
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_AND_USAGE_REPORT"].TableProperties[*]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_AND_USAGE_REPORT"].TableProperties[*].Description
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_AND_USAGE_REPORT"].TableProperties[*].ValidValues[*]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_OPTIMIZATION_RECOMMENDATIONS"].Description
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_OPTIMIZATION_RECOMMENDATIONS"].TableProperties[*]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="COST_OPTIMIZATION_RECOMMENDATIONS"].TableProperties[*].Description
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="FOCUS_1_0_AWS"].Description
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="FOCUS_1_0_AWS"].TableProperties[*]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="FOCUS_1_0_AWS_PREVIEW"]
Issue-Key: bcm-data-exports|ListTables|2xx->2xx|$.Tables[TableName="FOCUS_1_2_AWS"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_bill_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_entity"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_end_date"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_billing_period_start_date"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoice_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_invoicing_entity"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="bill_payer_account_name"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="cost_category"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="discount"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_line_item_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="identity_time_interval"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_availability_zone"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_blended_rate"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_currency_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_legal_entity"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_description"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_line_item_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_net_unblended_rate"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalization_factor"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_normalized_usage_amount"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_operation"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_product_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_tax_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_unblended_rate"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_account_name"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_amount"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_end_date"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_start_date"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="line_item_usage_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_currency"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_lease_contract_length"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_offering_class"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_public_on_demand_rate"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_purchase_option"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_rate_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_term"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="pricing_unit"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_comment"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_fee_description"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_location_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_from_region_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_family"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instance_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_instancesku"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_location_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_operation"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_pricing_unit"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_product_family"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_region_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_servicecode"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_sku"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_location_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_to_region_code"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="product_usagetype"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_cost_for_usage"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_amortized_upfront_fee_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_availability_zone"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_effective_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_end_time"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_modification_status"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_cost_for_usage"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_amortized_upfront_fee_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_effective_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_recurring_fee_for_usage"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_amortized_upfront_fee_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_unused_recurring_fee"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_net_upfront_value"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_normalized_units_per_reservation"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_number_of_reservations"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_recurring_fee_for_usage"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_reservation_a_r_n"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_start_time"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_subscription_id"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_normalized_units"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_total_reserved_units"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_units_per_reservation"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_amortized_upfront_fee_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_normalized_unit_quantity"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_quantity"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_unused_recurring_fee"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="reservation_upfront_value"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_amortized_upfront_commitment_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_end_time"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_instance_type_family"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_amortized_upfront_commitment_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_recurring_commitment_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_net_savings_plan_effective_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_offering_type"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_payment_option"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_purchase_term"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_recurring_commitment_for_billing_period"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_region"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_a_r_n"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_effective_cost"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_savings_plan_rate"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_start_time"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_total_commitment_to_date"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.Schema[Name="savings_plan_used_commitment"]
Issue-Key: bcm-data-exports|GetTable|2xx->2xx|$.TableProperties
Issue-Commit: f95a296744f71657ae3614221270df79d0b4a393

# GetTable and ListTables catalog metadata diverges from AWS

> _Note: this issue was identified against the code base at commit `f95a296744f71657ae3614221270df79d0b4a393`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
The emulator's built-in data-exports table catalog (the static catalog backing GetTable/ListTables) is not the AWS catalog: it is missing tables, renders column types in the wrong vocabulary, omits every column description, substitutes its own table/table-property prose, and resolves at least one table name to the wrong table. 7 causes, all in the one catalog data source and the GetTable/ListTables serializers.

Cause 1 of 7: GetTable(TableName=CARBON_EMISSIONS) is rejected as an unknown table.
AWS: Smithy GetTable takes a required TableName (GetTableRequest.TableName) and returns 200 with TableName/Description/Schema (ColumnList)/TableProperties; its only client error is the service-common ValidationException (@httpError 400). The recording shows AWS answering 200 for CARBON_EMISSIONS with Description 'Carbon Emissions' and a Schema containing payer_account_id, usage_account_id, usage_period_start, usage_period_end, last_refresh_timestamp, product_code, region_code, location, ... i.e. CARBON_EMISSIONS is a real table in the catalog.
Emulator: HTTP 400; no Description, no Schema, no TableName in the body.
Why: a client enumerating or exporting carbon-emissions data cannot discover the table's columns at all, and a CreateExport built from that schema cannot be constructed.
Fix: the GetTable handler's table-catalog data (add the CARBON_EMISSIONS table definition); no handler-logic change needed beyond the catalog entry.
Traces: bcm-data-exports-catalog-get-table#task#1 (cluster also covers #task#2).

Cause 2 of 7: GetTable(COST_AND_USAGE_REPORT) returns the wrong Description, the wrong Column.Type vocabulary, and no Column.Description.
AWS: Smithy Column has Name, Type ('The kind of data a column stores') and Description members. The recording shows Type values rendered as 'String'/'Number'/'Timestamp' (mixed case) and every column carrying a Description, and the table Description being the short catalog label 'Cost and Usage Report'.
Emulator: Type is rendered upper-case ('STRING' instead of 'String'); Column.Description is absent for every column; the table Description is the emulator's own long prose ('The Cost and Usage Report table contains the most detailed set of cost and usage data available for an account, with one row per line item.'). Columns discount_bundled_discount, discount_total_discount, line_item_user_identifier, resource_tags and tags are also absent/shaped differently.
Why: Column.Type is a value clients branch on when generating SQL/DDL from the catalog; 'STRING' is not a type AWS ever emits, so type-driven code paths mis-dispatch, and column documentation is simply unavailable.
Fix: the GetTable serializer's Type casing plus the catalog's per-column Description/column list.
Traces: bcm-data-exports-catalog-get-table#task#3.

Cause 3 of 7: GetTable(COST_OPTIMIZATION_RECOMMENDATIONS) — same Type-casing and missing-Description fault, plus a missing column and a spurious table property.
AWS: recorded behavior shows Description 'Cost Optimization Recommendations from the Cost Optimization Hub.', columns typed 'String' etc. with Descriptions, and a column account_name ('The account name that the recommendation is for.').
Emulator: upper-case Types, no column Descriptions, account_name absent, recommendation_lookback_period_in_days shaped differently, and a TableProperties.FILTER entry AWS does not return.
Why: as cause 2, plus a consumer selecting account_name gets a column-not-found at export-creation time.
Fix: the GetTable catalog entry for COST_OPTIMIZATION_RECOMMENDATIONS and the shared Type serializer.
Traces: bcm-data-exports-catalog-get-table#task#4.

Cause 4 of 7: GetTable(FOCUS_1_0_AWS) — same fault across the FOCUS schema.
AWS: recorded Description 'FinOps Open Cost & Usage Specification'; column Types 'String', 'Number' (e.g. BilledCost), 'Timestamp' (e.g. BillingPeriodStart/End), each with a Description.
Emulator: Description replaced with its own prose, Types rendered 'STRING'/'DOUBLE'/'TIMESTAMP' ('DOUBLE' is not an AWS Column.Type value at all), Descriptions absent.
Why: FOCUS consumers map Number/Timestamp columns by type; 'DOUBLE' and 'TIMESTAMP' are unrecognised.
Fix: the Type serializer's value mapping (Number, not DOUBLE) and the FOCUS catalog entry.
Traces: bcm-data-exports-catalog-get-table#task#5.

Cause 5 of 7: GetTable for the preview/newer FOCUS tables is rejected as unknown.
AWS: the recording answers 200 with Description 'FinOps Open Cost & Usage Specification', a full FOCUS Schema and TableProperties for these table names.
Emulator: HTTP 400 with no Description/Schema/TableName/TableProperties.
Why: a client pinned to the FOCUS preview/1.2 table cannot read its schema and cannot create an export against it.
Fix: add the missing FOCUS table variants to the GetTable catalog.
Traces: bcm-data-exports-catalog-get-table#task#6 (cluster also covers #task#7).

Cause 6 of 7: ListTables returns a different catalog than AWS.
AWS: Smithy ListTables returns Tables (TableList) of table summaries, each with TableName, Description and TableProperties (Name/Description/ValidValues/DefaultValue). The recording lists CARBON_EMISSIONS, COST_AND_USAGE_DASHBOARD, COST_AND_USAGE_REPORT, COST_OPTIMIZATION_RECOMMENDATIONS, FOCUS_1_0_AWS, FOCUS_1_0_AWS_PREVIEW and FOCUS_1_2_AWS, and for COST_AND_USAGE_REPORT the first property is TIME_GRANULARITY ('The granularity of the line-items in the table.') with ValidValues DAILY/MONTHLY/HOURLY, followed by INCLUDE_CAPACITY_RESERVATION_DATA and BILLING_VIEW_ARN.
Emulator: CARBON_EMISSIONS, COST_AND_USAGE_DASHBOARD, FOCUS_1_0_AWS_PREVIEW and FOCUS_1_2_AWS are absent from Tables; the per-table Descriptions are the emulator's prose; and the TableProperties list is a different set in a different order (its first property is a TRUE/FALSE discount-compatibility flag, and INCLUDE_RESOURCES / INCLUDE_SPLIT_COST_ALLOCATION_DATA appear where AWS returns INCLUDE_CAPACITY_RESERVATION_DATA / BILLING_VIEW_ARN).
Why: ListTables is the discovery call; clients that enumerate tables and then offer their properties see the wrong table set and the wrong property names and ValidValues, so a TIME_GRANULARITY=HOURLY export cannot be composed from discovered metadata.
Fix: the ListTables handler's catalog data — it must be the same catalog GetTable serves, including TableProperties names, descriptions, ValidValues and order.
Traces: bcm-data-exports-catalog-list-tables#task#1.

Cause 7 of 7: GetTable resolves a table name to the wrong table.
AWS: GetTable returns the metadata of the table named in the required TableName member; the recorded step requests the Cost and Usage Report table and AWS answers Description 'Cost and Usage Report' with the identity_*/bill_*/line_item_*/pricing_*/product_*/reservation_*/savings_plan_* CUR columns.
Emulator: answers 200 but with the FOCUS 1.0 table instead — Description is the emulator's FOCUS prose and every CUR column (identity_line_item_id, identity_time_interval, bill_invoice_id, ... savings_plan_used_commitment) is missing, as is TableProperties.
Why: this is silent wrong-data: the call succeeds, so a client builds a query against a schema that is not the table it asked for and the export fails or returns the wrong columns later.
Fix: the GetTable handler's table-name lookup (name-to-table resolution), not the catalog contents.
Traces: bcm-data-exports-table-catalog-inspection#task#2.

## Evidence
Replayed against the emulator booted from mainline (emulator_up=true, state_reset_failures=0, 10 fixtures replayed, 49 steps, 10 passed).

Cause 1 — bcm-data-exports-catalog-get-table#task#1, GetTable: expected status 200, got 400; $.Description missing (expected 'Carbon Emissions'); $.Schema missing (expected the payer_account_id/usage_account_id/usage_period_start/usage_period_end/last_refresh_timestamp/product_code/region_code/location column list); $.TableName missing. affected_count=2 (also #task#2).

Cause 2 — #task#3, GetTable 200 vs 200: $.Description value_mismatch (expected 'Cost and Usage Report', got 'The Cost and Usage Report table contains the most detailed set of cost and usage data available for an account, with one row per line item.'); $.Schema[Name="identity_line_item_id"].Type value_mismatch (expected 'String', got 'STRING'); $.Schema[Name="identity_line_item_id"].Description missing; same Type/Description divergence repeated for identity_time_interval, bill_invoice_id, bill_invoicing_entity, bill_billing_entity, bill_bill_type, bill_payer_account_id, line_item_*, pricing_*, product_*, reservation_*, savings_plan_*; $.Schema[Name="discount_bundled_discount"], ["discount_total_discount"], ["line_item_user_identifier"], ["resource_tags"], ["tags"] divergent/missing.

Cause 3 — #task#4, GetTable 200 vs 200: $.Description value_mismatch (expected 'Cost Optimization Recommendations from the Cost Optimization Hub.'); $.Schema[Name="account_name"] missing (expected {'Description': 'The account name that the recommendation is for.', 'Name': 'account_name', 'Type': 'String'}); $.Schema[Name="account_id"].Type expected 'String' got 'STRING' (same for recommendation_id, resource_arn, region, current_resource_type, recommended_resource_type, action_type, ...); $.TableProperties.FILTER divergent.

Cause 4 — #task#5, GetTable 200 vs 200: $.Description value_mismatch (expected 'FinOps Open Cost & Usage Specification'); $.Schema[Name="BilledCost"].Type expected 'Number' got 'DOUBLE'; $.Schema[Name="BillingPeriodEnd"].Type expected 'Timestamp' got 'TIMESTAMP'; $.Schema[Name="AvailabilityZone"].Type expected 'String' got 'STRING'; Description missing on every FOCUS column.

Cause 5 — #task#6, GetTable: expected 200, got 400; $.Description, $.Schema, $.TableName and $.TableProperties all missing (expected the full FOCUS column list). affected_count=2 (also #task#7).

Cause 6 — bcm-data-exports-catalog-list-tables#task#1, ListTables 200 vs 200: $.Tables[TableName="CARBON_EMISSIONS"] missing; $.Tables[TableName="COST_AND_USAGE_DASHBOARD"] missing; $.Tables[TableName="FOCUS_1_0_AWS_PREVIEW"] missing; $.Tables[TableName="FOCUS_1_2_AWS"] missing; $.Tables[TableName="COST_AND_USAGE_REPORT"].TableProperties[0].ValidValues[0] expected 'DAILY' got 'TRUE', [1] expected 'MONTHLY' got 'FALSE', [2] expected 'HOURLY' got None; TableProperties[1] expected INCLUDE_CAPACITY_RESERVATION_DATA got INCLUDE_RESOURCES; TableProperties[2] expected BILLING_VIEW_ARN got INCLUDE_SPLIT_COST_ALLOCATION_DATA.

Cause 7 — bcm-data-exports-table-catalog-inspection#task#2, GetTable 200 vs 200: $.Description value_mismatch (expected 'Cost and Usage Report', got the FOCUS 1.0 prose); $.Schema[Name="identity_line_item_id"], ["identity_time_interval"], ["bill_invoice_id"], ["bill_invoicing_entity"], ["bill_billing_entity"], ["bill_bill_type"], ["bill_payer_account_id"] ... through ["savings_plan_used_commitment"] and $.TableProperties all missing.

Model consulted: services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/smithy-model — GetTable(TableName required, output TableName/Description/TableProperties/Schema:ColumnList, errors ValidationException 400), ListTables(output Tables:TableList, paginated), Column{Name,Type,Description}.

## Steps to Reproduce
1. git clone the mainline checkout and boot the emulator from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture bcm-data-exports-catalog-get-table, step task#1 — GetTable with TableName=CARBON_EMISSIONS; observe 400 instead of 200 with Description/Schema.
4. Cause 2: same fixture, step task#3 — GetTable for COST_AND_USAGE_REPORT; observe Column.Type 'STRING' instead of 'String', no Column.Description, and the substituted table Description.
5. Cause 3: same fixture, step task#4 — GetTable for COST_OPTIMIZATION_RECOMMENDATIONS; observe the missing account_name column and upper-case Types.
6. Cause 4: same fixture, step task#5 — GetTable for FOCUS_1_0_AWS; observe Type 'DOUBLE'/'TIMESTAMP' instead of 'Number'/'Timestamp'.
7. Cause 5: same fixture, step task#6 — GetTable for the FOCUS preview/1.2 table; observe 400 instead of 200.
8. Cause 6: replay fixture bcm-data-exports-catalog-list-tables, step task#1 — ListTables; observe the four missing tables and the different TableProperties set/ValidValues.
9. Cause 7: replay fixture bcm-data-exports-table-catalog-inspection, step task#2 — GetTable for the Cost and Usage Report table; observe the FOCUS 1.0 table returned instead.
