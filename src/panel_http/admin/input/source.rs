use super::*;
use crate::catalog::{GlmQuotaProfile, SecretString, UpstreamProfile};
use crate::config::{HeaderKV, ModelDiscoveryAuth, ModelDiscoveryConfig};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    key: String,
    value: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    url: String,
    #[serde(default, deserialize_with = "string_enum")]
    auth: ModelDiscoveryAuth,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upstream {
    channel_type: i64,
    base_url: String,
    #[serde(default, deserialize_with = "optional_object")]
    model_discovery: Option<Discovery>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Quota {
    quota_url: String,
    #[serde(default, deserialize_with = "objects")]
    global_headers: Vec<Header>,
    #[serde(default, deserialize_with = "objects")]
    key_headers: Vec<Header>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Settings {
    Glm {
        #[serde(deserialize_with = "object")]
        upstream: Upstream,
        #[serde(deserialize_with = "object")]
        quota: Quota,
    },
    Volcano {
        #[serde(deserialize_with = "object")]
        upstream: Upstream,
        #[serde(default, deserialize_with = "object")]
        policy: PolicyInput,
    },
}
#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum Selection {
    Discover {},
    Manual { manual_models: Vec<String> },
    DiscoverOrManual { manual_models: Vec<String> },
}
impl Default for Selection {
    fn default() -> Self {
        Self::Discover {}
    }
}
body!(Source {
    name: String,
    #[serde(default)]
    note: String,
    credential: String,
    #[serde(deserialize_with = "object")]
    settings: Settings,
    #[serde(default, deserialize_with = "object")]
    model_selection: Selection
});
pub(in crate::panel_http::admin) enum ModelSelection {
    Discover,
    Manual(Vec<String>),
    DiscoverOrManual(Vec<String>),
}
pub(in crate::panel_http::admin) enum SourceKindInput {
    Glm(GlmQuotaProfile),
    Volcano(VolcanoPolicy),
}
pub(in crate::panel_http::admin) struct NewSource {
    pub(in crate::panel_http::admin) name: String,
    pub(in crate::panel_http::admin) note: String,
    pub(in crate::panel_http::admin) credential: SecretString,
    pub(in crate::panel_http::admin) upstream: UpstreamProfile,
    pub(in crate::panel_http::admin) kind: SourceKindInput,
    pub(in crate::panel_http::admin) selection: ModelSelection,
}
fn url(text: &str, query: bool) -> Result<(), AdminFailure> {
    if text.len() > 8192 {
        return Err(invalid_input());
    }
    crate::catalog::types::validate_url(text, query)?;
    Ok(())
}
fn headers(values: Vec<Header>) -> Result<Vec<HeaderKV>, AdminFailure> {
    if values.len() > 64 {
        return Err(invalid_input());
    }
    values
        .into_iter()
        .map(|v| {
            reqwest::header::HeaderName::from_bytes(v.key.as_bytes())
                .map_err(|_| invalid_input())?;
            reqwest::header::HeaderValue::from_str(&v.value).map_err(|_| invalid_input())?;
            Ok(HeaderKV {
                key: v.key,
                value: v.value,
            })
        })
        .collect()
}
pub(super) fn decode(bytes: &[u8]) -> Result<NewSource, AdminFailure> {
    let d: Source = json(bytes)?;
    let name = name(d.name)?;
    if d.note.chars().count() > 2048 || d.note.contains('\0') {
        return Err(invalid_input());
    }
    let credential = SecretString::new(d.credential)?;
    let selection = match d.model_selection {
        Selection::Discover {} => ModelSelection::Discover,
        Selection::Manual { manual_models } => ModelSelection::Manual(models(manual_models)?),
        Selection::DiscoverOrManual { manual_models } => {
            ModelSelection::DiscoverOrManual(models(manual_models)?)
        }
    };
    let (upstream, kind) = match d.settings {
        Settings::Glm { upstream, quota } => {
            url(&quota.quota_url, true)?;
            (
                upstream,
                SourceKindInput::Glm(GlmQuotaProfile {
                    quota_url: quota.quota_url,
                    global_headers: headers(quota.global_headers)?,
                    key_headers: headers(quota.key_headers)?,
                }),
            )
        }
        Settings::Volcano { upstream, policy } => {
            (upstream, SourceKindInput::Volcano(policy.validate()?))
        }
    };
    let channel_type = revision(upstream.channel_type)?;
    url(&upstream.base_url, false)?;
    let model_discovery = upstream
        .model_discovery
        .map(|d| {
            url(&d.url, false)?;
            Ok::<_, AdminFailure>(ModelDiscoveryConfig {
                url: d.url,
                auth: d.auth,
            })
        })
        .transpose()?;
    if !matches!(&selection, ModelSelection::Manual(_)) && model_discovery.is_none() {
        return Err(bad_request());
    }
    Ok(NewSource {
        name,
        note: d.note,
        credential,
        kind,
        selection,
        upstream: UpstreamProfile {
            channel_type,
            base_url: upstream.base_url,
            model_discovery,
            models: Vec::new(),
        },
    })
}
