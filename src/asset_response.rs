use getset::Getters;
use serde::Deserialize;

#[derive(Debug, Deserialize, Getters, Clone)]
#[getset(get = "pub")]
#[serde(rename_all = "camelCase")]
pub struct AssetResponse {
    pub locations: Vec<Location>,
    #[allow(dead_code)]
    pub request_id: String,
    #[allow(dead_code)]
    pub is_archived: bool,
    #[allow(dead_code)]
    pub asset_type_id: u64,
    #[allow(dead_code)]
    pub is_recordable: bool,
}

#[derive(Debug, Deserialize, Getters, Clone)]
#[getset(get = "pub")]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub asset_format: String,
    pub location: String,
    #[allow(dead_code)]
    pub asset_metadatas: Vec<AssetMetadata>,
}

#[derive(Debug, Deserialize, Getters, Clone)]
#[getset(get = "pub")]
#[serde(rename_all = "camelCase")]
pub struct AssetMetadata {
    #[allow(dead_code)]
    pub metadata_type: u64,
    #[allow(dead_code)]
    pub value: String,
}
