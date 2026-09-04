//! OpenAI Admin API types — the Costs API
//! (`GET /v1/organization/costs`) answers daily billed dollars grouped by
//! line item. The response is a paged list of `(day, line_item, amount)`
//! rows; this module only models the tolerant read (missing/null
//! `line_item`, unknown currencies) — aggregation lives in `fetch`.

use chrono::NaiveDate;
use serde::Deserialize;

/// One row of the costs list: a day's spend for one line item.
#[derive(Debug, Clone, Deserialize)]
pub struct CostRow {
    /// Unix seconds of the bucket start.
    pub start_time: i64,
    #[serde(default)]
    pub amount: Option<Amount>,
    /// Null when the request omitted `group_by` (rows lose their item then)
    /// or for fees without a model.
    #[serde(default)]
    pub line_item: Option<LineItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Amount {
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LineItem {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CostsPage {
    #[serde(default)]
    pub data: Vec<CostRow>,
    #[serde(default)]
    pub has_next_page: bool,
    #[serde(default)]
    pub next_page: Option<String>,
}

/// The date of a unix-seconds bucket start, in UTC.
pub fn bucket_date(start_time: i64) -> Option<NaiveDate> {
    use chrono::TimeZone;
    chrono::Utc
        .timestamp_opt(start_time, 0)
        .single()
        .map(|at| at.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai_api::fetch::CostAggregate;

    #[test]
    fn costs_page_parses_the_modeled_flat_row_shape() {
        let page: CostsPage = serde_json::from_str(
            r#"{
              "object": "page",
              "data": [
                {
                  "object": "organization.costs.bucket",
                  "id": "bc_688b5d8c",
                  "start_time": 1754006400,
                  "end_time": 1754092799,
                  "project_id": "proj_abc",
                  "aggregation_result": null,
                  "amount": {"value": 1.23, "currency": "USD"},
                  "line_item": {"name": "gpt-5.2"}
                },
                {
                  "object": "organization.costs.bucket",
                  "id": "bc_688b5d8d",
                  "start_time": 1754092800,
                  "end_time": 1754179199,
                  "project_id": "proj_abc",
                  "aggregation_result": null,
                  "amount": {"value": 0.0, "currency": "USD"},
                  "line_item": null
                }
              ],
              "has_next_page": true,
              "next_page": "cursor_ord-2"
            }"#,
        )
        .unwrap();
        assert_eq!(page.data.len(), 2);
        assert_eq!(page.data[0].start_time, 1754006400);
        assert_eq!(page.data[0].amount.as_ref().unwrap().value, Some(1.23));
        assert_eq!(
            page.data[0].line_item.as_ref().unwrap().name.as_deref(),
            Some("gpt-5.2")
        );
        // serde must keep ignoring upstream fields these structs don't model.
        assert!(page.data[1].line_item.is_none());
        assert!(page.has_next_page);
        assert_eq!(page.next_page.as_deref(), Some("cursor_ord-2"));
    }

    #[test]
    fn the_documented_upstream_envelope_degrades_to_an_empty_window() {
        // developers.openai.com costs response: `has_more` (not
        // `has_next_page`) and nested `bucket.results` rows. The tolerant read
        // swallows that shape without error — pagination stops and every row
        // degrades to amount=None — which is exactly why this pin exists:
        // changing the DTO to match upstream must flip this test.
        let page: CostsPage = serde_json::from_str(
            r#"{
              "object": "page",
              "data": [
                {
                  "object": "bucket",
                  "start_time": 1754006400,
                  "end_time": 1754092800,
                  "results": [
                    {
                      "object": "organization.costs.result",
                      "amount": {"value": 0.06, "currency": "usd"},
                      "line_item": "gpt-5.2",
                      "project_id": null,
                      "api_key_id": null,
                      "quantity": null
                    }
                  ]
                }
              ],
              "has_more": false,
              "next_page": null
            }"#,
        )
        .unwrap();
        assert!(!page.has_next_page && page.next_page.is_none());
        assert!(page.data.iter().all(|row| row.amount.is_none()));
    }

    #[test]
    fn missing_and_null_fields_default_tolerantly() {
        let page: CostsPage = serde_json::from_str("{}").unwrap();
        assert!(page.data.is_empty());
        assert!(!page.has_next_page);
        assert!(page.next_page.is_none());

        let row: CostRow = serde_json::from_str(r#"{"start_time": 1754006400}"#).unwrap();
        assert_eq!(row.start_time, 1754006400);
        assert!(row.amount.is_none());
        assert!(row.line_item.is_none());
    }

    #[test]
    fn wrong_field_types_are_rejected_not_tolerated() {
        assert!(
            serde_json::from_str::<CostRow>(
                r#"{"start_time": 1754006400, "amount": {"value": "1.23"}}"#
            )
            .is_err()
        );
        assert!(serde_json::from_str::<CostRow>(r#"{"start_time": "1754006400"}"#).is_err());
    }

    #[test]
    fn bucket_date_out_of_range_is_none() {
        assert_eq!(
            bucket_date(1754006400),
            chrono::NaiveDate::from_ymd_opt(2025, 8, 1)
        );
        assert_eq!(bucket_date(i64::MAX), None);
        assert_eq!(bucket_date(i64::MIN), None);
    }

    #[test]
    fn cost_aggregate_round_trips_through_the_cache_envelope() {
        let aggregate = CostAggregate {
            spent: 4.2,
            by_day: vec![(chrono::NaiveDate::from_ymd_opt(2025, 8, 1).unwrap(), 1.5)],
            top_items: vec![("gpt-5.2".into(), 3.5)],
        };
        let text = serde_json::to_string(&aggregate).unwrap();
        let parsed: CostAggregate = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.spent, 4.2);
        assert_eq!(
            parsed.by_day,
            vec![(chrono::NaiveDate::from_ymd_opt(2025, 8, 1).unwrap(), 1.5)]
        );
        assert_eq!(parsed.top_items, vec![("gpt-5.2".to_string(), 3.5)]);
    }
}
