use crate::models::{CompMeta, Competition, Config, FileEntry, RecentComp, Stats, Tag};
use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

pub const PORT: u16 = 3000;
pub const DATA_DIR: &str = "src";
pub const METADATA_FILE: &str = "src/metadata.json";
pub const LEGACY_FILE: &str = "src/awards.json";
pub const CONFIG_FILE: &str = "config.json";

pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn default_config() -> Config {
    Config {
        admin_password_hash: hash_password("admin"),
        session_token: String::new(),
        site_title: "Modeling Papers".to_string(),
        site_subtitle: "Mathematical Modeling Competition Collection".to_string(),
    }
}

pub fn load_config() -> Config {
    let mut config = default_config();
    if let Ok(data) = fs::read_to_string(CONFIG_FILE)
        && let Ok(mut loaded) = serde_json::from_str::<Config>(&data)
    {
        if loaded.admin_password_hash.is_empty() {
            loaded.admin_password_hash = config.admin_password_hash;
        }
        if loaded.site_title.is_empty() {
            loaded.site_title = config.site_title;
        }
        if loaded.site_subtitle.is_empty() {
            loaded.site_subtitle = config.site_subtitle;
        }
        config = loaded;
    }
    config
}

pub fn save_config(config: &Config) -> io::Result<()> {
    let data = serde_json::to_vec_pretty(config)?;
    fs::write(CONFIG_FILE, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(CONFIG_FILE, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn load_meta() -> HashMap<String, CompMeta> {
    fs::read_to_string(METADATA_FILE)
        .ok()
        .and_then(|data| serde_json::from_str::<HashMap<String, CompMeta>>(&data).ok())
        .unwrap_or_default()
}

pub fn save_meta(meta: &HashMap<String, CompMeta>) -> io::Result<()> {
    let data = serde_json::to_vec_pretty(meta)?;
    fs::write(METADATA_FILE, data)
}

pub fn migrate_legacy_awards(meta: &mut HashMap<String, CompMeta>) {
    let Ok(data) = fs::read_to_string(LEGACY_FILE) else {
        return;
    };
    let Ok(awards) = serde_json::from_str::<HashMap<String, String>>(&data) else {
        return;
    };

    let mut changed = false;
    for (name, award) in awards {
        if award.is_empty() {
            continue;
        }
        let comp = meta.entry(name).or_insert_with(|| CompMeta {
            status: "completed".to_string(),
            tags: Vec::new(),
            order: None,
        });
        let exists = comp
            .tags
            .iter()
            .any(|tag| tag.is_award && tag.text == award);
        if !exists {
            comp.tags.push(Tag {
                text: award,
                is_award: true,
                level: "national".to_string(),
            });
            changed = true;
        }
    }

    if changed {
        let _ = save_meta(meta);
    }
    let _ = fs::rename(LEGACY_FILE, format!("{LEGACY_FILE}.migrated"));
}

pub fn list_competitions() -> Vec<Competition> {
    let Ok(entries) = fs::read_dir(DATA_DIR) else {
        return Vec::new();
    };
    let meta = load_meta();
    let mut competitions = Vec::new();

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let dir_path = entry.path();
        let mut files = Vec::new();
        let mut has_thesis = false;

        if let Ok(children) = fs::read_dir(&dir_path) {
            for child in children.flatten() {
                if child
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
                {
                    let file_name = child.file_name().to_string_lossy().to_string();
                    if file_name == "thesis.pdf" {
                        has_thesis = true;
                    }
                    files.push(file_name);
                }
            }
        }

        let modified_time = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map(rfc3339)
            .unwrap_or_else(|_| rfc3339(UNIX_EPOCH));

        let comp_meta = meta.get(&name);
        let status = comp_meta
            .and_then(|item| (!item.status.is_empty()).then(|| item.status.clone()))
            .unwrap_or_else(|| "completed".to_string());
        let tags = comp_meta.map(|item| item.tags.clone()).unwrap_or_default();
        let order = comp_meta.and_then(|item| item.order).unwrap_or(usize::MAX);

        competitions.push(Competition {
            name,
            file_count: files.len(),
            files,
            has_thesis,
            status,
            tags,
            modified_time,
            total_size: dir_size(&dir_path),
            order,
        });
    }

    competitions.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| right.modified_time.cmp(&left.modified_time))
            .then_with(|| left.name.cmp(&right.name))
    });
    competitions
}

pub fn list_files(name: &str) -> io::Result<Vec<FileEntry>> {
    let base = competition_dir(name)?;
    if !base.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "not found"));
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(&base).into_iter().filter_map(Result::ok) {
        if entry.path() == base {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(&base) else {
            continue;
        };
        let path = path_to_slash(rel);
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        files.push(FileEntry {
            path,
            size: metadata.len(),
            is_dir: metadata.is_dir(),
            modified_time: metadata.modified().map(rfc3339).unwrap_or_default(),
        });
    }
    Ok(files)
}

pub fn build_stats() -> Stats {
    let competitions = list_competitions();
    let mut stats = Stats {
        total_competitions: 0,
        total_files: 0,
        total_size: 0,
        by_status: BTreeMap::new(),
        by_award_level: BTreeMap::new(),
        recent_updated: Vec::new(),
    };

    for competition in &competitions {
        stats.total_competitions += 1;
        stats.total_files += competition.file_count;
        stats.total_size += dir_size(Path::new(DATA_DIR).join(&competition.name));
        *stats
            .by_status
            .entry(competition.status.clone())
            .or_default() += 1;

        for tag in &competition.tags {
            if tag.is_award {
                let level = if tag.level.is_empty() {
                    "national"
                } else {
                    &tag.level
                };
                *stats.by_award_level.entry(level.to_string()).or_default() += 1;
            }
        }
    }

    stats.recent_updated = competitions
        .into_iter()
        .take(5)
        .map(|competition| RecentComp {
            name: competition.name,
            status: competition.status,
            file_count: competition.file_count,
            modified_time: competition.modified_time,
        })
        .collect();
    stats
}

pub fn competition_dir(name: &str) -> io::Result<PathBuf> {
    let base = normalized_absolute(Path::new(DATA_DIR))?;
    let candidate = normalized_absolute(Path::new(DATA_DIR).join(name))?;
    ensure_child(&base, &candidate)?;
    Ok(candidate)
}

pub fn competition_child(name: &str, rel_path: &str) -> io::Result<PathBuf> {
    let base = competition_dir(name)?;
    let candidate = normalized_absolute(base.join(Path::new(rel_path)))?;
    ensure_child(&base, &candidate)?;
    Ok(candidate)
}

pub fn competition_child_path(name: &str, rel_path: &Path) -> io::Result<PathBuf> {
    let base = competition_dir(name)?;
    let candidate = normalized_absolute(base.join(rel_path))?;
    ensure_child(&base, &candidate)?;
    Ok(candidate)
}

pub fn dir_size(path: impl AsRef<Path>) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| entry.metadata().ok())
        .filter(|metadata| metadata.is_file())
        .map(|metadata| metadata.len())
        .sum()
}

pub fn path_to_slash(path: impl AsRef<Path>) -> String {
    path.as_ref()
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn normalized_absolute(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    Ok(normalized)
}

fn ensure_child(base: &Path, candidate: &Path) -> io::Result<()> {
    if candidate.starts_with(base) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path outside competition",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_go_sha256_hex() {
        assert_eq!(
            hash_password("admin"),
            "8c6976e5b5410415bde908bd4dee15dfb167a9c873fc4bb8a81f6f2ab448a918"
        );
    }

    #[test]
    fn relative_child_paths_stay_inside_base() {
        assert!(competition_child("sample", "nested/file.pdf").is_ok());
        assert!(competition_child("sample", "../metadata.json").is_err());
    }
}
