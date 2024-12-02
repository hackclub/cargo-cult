use std::env;
use std::error::Error;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FormData {
    #[serde(rename = "Type")]
    pub submission_type: String, // Submission | Update
    
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Slack Handle")]
    pub slack_handle: String,
    #[serde(rename = "Email")]
    pub email: String,

    #[serde(rename = "Address Line 1")]
    pub address_line1: String,
    #[serde(rename = "Address Line 2")]
    #[serde(default)]
    pub address_line2: String,
    #[serde(rename = "City")]
    pub city: String,
    #[serde(rename = "State")]
    pub state: String,
    #[serde(rename = "Zip")]
    pub zip: String,
    #[serde(rename = "Country")]
    pub country: String,

    #[serde(rename = "Package Link")]
    pub package_link: String,
    #[serde(rename = "Description")]
    pub description: String,
    #[serde(rename = "Hours")]
    pub hours: String,

    #[serde(rename = "Package Name")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_name: Option<String>
}

impl FormData {
    pub fn new() -> Self {
        Self {
            submission_type: "Submission".to_string(),
            name: "".to_string(),
            slack_handle: "".to_string(),
            email: "".to_string(),
            address_line1: "".to_string(),
            address_line2: "".to_string(),
            city: "".to_string(),
            state: "".to_string(),
            zip: "".to_string(),
            country: "".to_string(),
            package_link: "".to_string(),
            description: "".to_string(),
            hours: "".to_string(),
            package_name: None
        }
    }
}

pub struct SubmissionsAirtableBase {
    client: reqwest::Client,
    airtable_key: String,
    base_id: String,
    table_name: String,
    view_name: String,
}

const BASE_ID: &str = "appLSCQFAClFemq86";
const TABLE_NAME: &str = "GA";
const VIEW_NAME: &str = "Approved";

// struct taken from the airtable-api crate
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record<T> {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    id: String,
    fields: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_time: Option<DateTime<Utc>>,
}

const AIRTABLE_BASE_URL: &str = "https://api.airtable.com/v0";

#[derive(Debug, Serialize, Deserialize)]
struct AirtableRecordsData {
    records: Vec<Record<FormData>>
}

impl SubmissionsAirtableBase {
    pub fn new() -> Self {
        let airtable_key = env::var("AIRTABLE_KEY").expect("AIRTABLE_KEY to be set");
        let client = reqwest::Client::new();

        Self {
            client,
            airtable_key,
            base_id: BASE_ID.into(),
            table_name: TABLE_NAME.into(),
            view_name: VIEW_NAME.into(),
        }
    }

    pub async fn get(&mut self) -> Result<Vec<FormData>, Box<dyn Error>> {

        let AirtableRecordsData { records } = self.client
            .get(format!("{AIRTABLE_BASE_URL}/{}/{}?maxRecords=100&view={}", self.base_id, self.table_name, self.view_name))
            .header("Authorization", format!("Bearer {}", self.airtable_key))
            .send().await?.json().await?;

        Ok(records.iter().map(|record| record.fields.clone()).collect())
    }

    pub async fn create(&mut self, data: FormData) -> reqwest::Result<()> {
        self.client
            .post(format!("{AIRTABLE_BASE_URL}/{}/{}", self.base_id, self.table_name))
            .header("Authorization", format!("Bearer {}", self.airtable_key))
            .header("Content-Type", "application/json")
            .json(&AirtableRecordsData {records: vec![Record {
                id: String::new(), fields: data, created_time: None
            }] }).send().await?;
        Ok(())
    }
}
