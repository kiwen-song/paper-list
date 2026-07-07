use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Tag {
    pub text: String,
    #[serde(rename = "isAward")]
    pub is_award: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct CompMeta {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<usize>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Competition {
    pub name: String,
    #[serde(rename = "fileCount")]
    pub file_count: usize,
    pub files: Vec<String>,
    #[serde(rename = "hasThesis")]
    pub has_thesis: bool,
    pub status: String,
    pub tags: Vec<Tag>,
    #[serde(rename = "modifiedTime")]
    pub modified_time: String,
    #[serde(rename = "totalSize")]
    pub total_size: u64,
    #[serde(default)]
    pub order: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Config {
    #[serde(rename = "adminPasswordHash", default)]
    pub admin_password_hash: String,
    #[serde(rename = "sessionToken", default)]
    pub session_token: String,
    #[serde(rename = "siteTitle", default)]
    pub site_title: String,
    #[serde(rename = "siteSubtitle", default)]
    pub site_subtitle: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    #[serde(rename = "isDir")]
    pub is_dir: bool,
    #[serde(rename = "modifiedTime")]
    pub modified_time: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecentComp {
    pub name: String,
    pub status: String,
    #[serde(rename = "fileCount")]
    pub file_count: usize,
    #[serde(rename = "modifiedTime")]
    pub modified_time: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Stats {
    #[serde(rename = "totalCompetitions")]
    pub total_competitions: usize,
    #[serde(rename = "totalFiles")]
    pub total_files: usize,
    #[serde(rename = "totalSize")]
    pub total_size: u64,
    #[serde(rename = "byStatus")]
    pub by_status: std::collections::BTreeMap<String, usize>,
    #[serde(rename = "byAwardLevel")]
    pub by_award_level: std::collections::BTreeMap<String, usize>,
    #[serde(rename = "recentUpdated")]
    pub recent_updated: Vec<RecentComp>,
}
