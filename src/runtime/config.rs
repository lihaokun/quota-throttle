use super::{RuntimeError, RuntimeResult};
use crate::config::Config;
use crate::panel_http::UiOptions;
use crate::reconcile::planner::SUPPORTED_BACKEND_VERSION;
use serde::Deserialize;
use std::io::Read;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Issuer {
    pub user_id: i64,
    pub username: String,
    pub password: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RuntimeOptions {
    pub catalog_path: PathBuf,
    pub panel_origin: Option<String>,
    pub public_api_base_url: Option<String>,
    pub issuer: Option<Issuer>,
    pub legacy_admin_user_id: Option<i64>,
}
#[derive(Deserialize)]
struct Document {
    source_groups: RuntimeOptions,
}

pub(super) struct Loaded {
    pub policy: Arc<Config>,
    pub options: RuntimeOptions,
    pub addr: SocketAddr,
    pub ui: UiOptions,
}
fn absolute_clean(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|p| matches!(p, Component::RootDir | Component::Normal(_)))
}
fn loopback(ip: IpAddr) -> bool {
    ip == IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        || ip == IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
}
pub(super) fn decode(text: &str) -> RuntimeResult<Loaded> {
    let bad = RuntimeError::Config;
    let policy: Config = toml::from_str(text).map_err(|_| bad)?;
    let options = toml::from_str::<Document>(text)
        .map_err(|_| bad)?
        .source_groups;
    crate::catalog::glm_round::validate_config(&policy).map_err(|_| bad)?;
    let addr: SocketAddr = policy.status_addr.parse().map_err(|_| bad)?;
    let manage = policy.new_api.manage.as_ref().ok_or(bad)?;
    let url = reqwest::Url::parse(&policy.new_api.base_url).map_err(|_| bad)?;
    let host = url.host_str().ok_or(bad)?;
    let native_loopback = host == "localhost" || host == "127.0.0.1" || host == "[::1]";
    if !cfg!(target_os = "linux")
        || !policy.keys.is_empty()
        || !(5..=3600).contains(&policy.poll_interval_secs)
        || !loopback(addr.ip())
        || addr.port() == 0
        || addr.port() == manage.port
        || !absolute_clean(&options.catalog_path)
        || options
            .catalog_path
            .file_name()
            .is_none_or(|v| v != "catalog.sqlite3")
        || !absolute_clean(Path::new(&manage.data_dir))
        || manage.port == 0
        || manage.version != SUPPORTED_BACKEND_VERSION
        || url.scheme() != "http"
        || !native_loopback
        || url.port_or_known_default() != Some(manage.port)
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || options.legacy_admin_user_id.is_some_and(|id| id <= 0)
        || options.issuer.as_ref().is_some_and(|i| {
            i.user_id <= 0
                || i.username.is_empty()
                || i.username.trim() != i.username
                || i.username == policy.new_api.root_username
                || i.password.is_empty()
                || Some(i.user_id) == options.legacy_admin_user_id
        })
    {
        return Err(bad);
    }
    let ui = UiOptions::new(options.public_api_base_url.clone()).map_err(|_| bad)?;
    Ok(Loaded {
        policy: Arc::new(policy),
        options,
        addr,
        ui,
    })
}
pub(super) fn load(path: &Path) -> RuntimeResult<Loaded> {
    // Step R1: 有界读取且所有解析错误脱敏；不输出包含密码的TOML错误行。
    let bad = RuntimeError::Config;
    let meta = std::fs::symlink_metadata(path).map_err(|_| bad)?;
    if !meta.is_file() || meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 {
        return Err(bad);
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|_| bad)?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut text)
        .map_err(|_| bad)?;
    if text.len() > 1024 * 1024 {
        return Err(bad);
    }
    decode(&text)
}
pub(super) fn private_directory(path: &Path) -> RuntimeResult<()> {
    if !absolute_clean(path) {
        return Err(RuntimeError::PrivatePath);
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| RuntimeError::PrivatePath)?;
        }
        Err(_) => return Err(RuntimeError::PrivatePath),
    }
    let meta = std::fs::symlink_metadata(path).map_err(|_| RuntimeError::PrivatePath)?;
    if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
        return Err(RuntimeError::PrivatePath);
    }
    Ok(())
}
