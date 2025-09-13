use getset::Getters;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct UniversePlacesResponse {
    #[serde(rename = "previousPageCursor")]
    #[get = "pub"]
    #[allow(dead_code)]
    previous_page_cursor: Option<String>,

    #[serde(rename = "nextPageCursor")]
    #[get = "pub"]
    #[allow(dead_code)]
    next_page_cursor: Option<String>,

    #[get = "pub"]
    data: Vec<Place>,
}

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct Place {
    #[get = "pub"]
    id: u64,

    #[serde(rename = "universeId")]
    #[get = "pub"]
    #[allow(dead_code)]
    universe_id: u64,

    #[get = "pub"]
    name: String,

    #[get = "pub"]
    #[allow(dead_code)]
    description: String,
}
