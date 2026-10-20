use super::route::{self, Route};
use super::{bad_request, invalid_input, AdminFailure};
use crate::catalog::control::{canonical_models, ControlCommand, OperationId, StatusDirective};
use crate::catalog::{
    GroupDraft, GroupId, ManagedTokenId, MembershipId, SourceId, SourceKind, VolcanoPolicy,
    JSON_SAFE,
};
use crate::ledger::analytics::UsageQuery;
use crate::newapi::tokens::TokenOwnerScope;
use serde::{Deserialize, Deserializer};
use std::time::Duration;

struct Version;
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if u8::deserialize(d)? == 1 {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("unsupported schema"))
        }
    }
}
macro_rules! body {
    ($name:ident { $($fields:tt)* }) => {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $name {
            #[serde(rename="schema_version")] _version: Version,
            $($fields)*
        }
    };
}
mod source;
mod usage;
pub(super) use source::{ModelSelection, NewSource, SourceKindInput};

fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
// 只交原始MapAccess，不经Value覆盖重复键；Serde普通struct也接受sequence，须在边界限形。
fn object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    struct ObjectVisitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for ObjectVisitor<T> {
        type Value = T;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("JSON object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(self, map: M) -> Result<T, M::Error> {
            T::deserialize(serde::de::value::MapAccessDeserializer::new(map))
        }
    }
    d.deserialize_map(ObjectVisitor(std::marker::PhantomData))
}
struct MapOnly<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for MapOnly<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        object(d).map(Self)
    }
}
fn optional_object<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<MapOnly<T>>::deserialize(d).map(|v| v.map(|v| v.0))
}
fn objects<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Vec::<MapOnly<T>>::deserialize(d).map(|v| v.into_iter().map(|v| v.0).collect())
}
fn string_enum<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    T::deserialize(serde::de::value::StringDeserializer::<D::Error>::new(
        String::deserialize(d)?,
    ))
}
fn json<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, AdminFailure> {
    if bytes.len() > crate::panel_http::request::MAX_BODY {
        return Err(AdminFailure::Http(
            crate::panel_http::HttpErrorCode::BodyTooLarge,
            None,
        ));
    }
    let mut parser = serde_json::Deserializer::from_slice(bytes);
    let value = object(&mut parser).map_err(|_| bad_request())?;
    parser.end().map_err(|_| bad_request())?;
    Ok(value)
}
fn safe(value: i64, min: i64) -> Result<i64, AdminFailure> {
    if (min..=JSON_SAFE).contains(&value) {
        Ok(value)
    } else {
        Err(invalid_input())
    }
}
fn revision(value: i64) -> Result<i64, AdminFailure> {
    safe(value, 1)
}
fn name(value: String) -> Result<String, AdminFailure> {
    let v = value.trim();
    if crate::catalog::types::valid_name(v) {
        Ok(v.to_owned())
    } else {
        Err(invalid_input())
    }
}
fn token_label(value: String) -> Result<String, AdminFailure> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(invalid_input())
    } else {
        Ok(value)
    }
}
fn models(mut value: Vec<String>) -> Result<Vec<String>, AdminFailure> {
    if value.iter().any(String::is_empty) {
        return Err(invalid_input());
    }
    canonical_models(&mut value)?;
    Ok(value)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyInput {
    #[serde(deserialize_with = "nullable")]
    daily_token_limit: Option<i64>,
    failure_threshold: i64,
    reset_minute: i64,
    utc_offset_minutes: i64,
}
impl Default for PolicyInput {
    fn default() -> Self {
        let p = VolcanoPolicy::default();
        Self {
            daily_token_limit: p.daily_token_limit,
            failure_threshold: p.failure_threshold,
            reset_minute: p.reset_minute,
            utc_offset_minutes: p.utc_offset_minutes,
        }
    }
}
impl PolicyInput {
    fn validate(self) -> Result<VolcanoPolicy, AdminFailure> {
        let value = VolcanoPolicy {
            daily_token_limit: self.daily_token_limit,
            failure_threshold: self.failure_threshold,
            reset_minute: self.reset_minute,
            utc_offset_minutes: self.utc_offset_minutes,
        };
        value.validate()?;
        Ok(value)
    }
}

pub(super) enum AdminRequest {
    CreateSource(NewSource),
    SourceModels(SourceId),
    Control {
        id: OperationId,
        command: ControlCommand,
    },
    DiscoverModels {
        id: OperationId,
        source: SourceId,
        revision: i64,
    },
    CreateGroup(GroupDraft),
    PlanMembership {
        source: SourceId,
        group: GroupId,
    },
    CheckGroup {
        group: GroupId,
        revision: i64,
        register: bool,
    },
    GroupReferences(GroupId),
    Operation(OperationId),
    Retry(OperationId),
    Identities(Option<ManagedTokenId>),
    Identity(ManagedTokenId),
    CreateToken {
        id: OperationId,
        group: GroupId,
        revision: i64,
        name: String,
        expires: Option<i64>,
    },
    SetTokenEnabled {
        id: OperationId,
        token: ManagedTokenId,
        revision: i64,
        enabled: bool,
    },
    DeleteToken {
        id: OperationId,
        token: ManagedTokenId,
        revision: i64,
    },
    RevealToken {
        token: ManagedTokenId,
        revision: i64,
    },
    NativeTokens {
        scope: TokenOwnerScope,
        page: usize,
    },
    RegisterToken {
        scope: TokenOwnerScope,
        native: i64,
        name: String,
    },
    Usage(UsageQuery),
}
impl AdminRequest {
    pub(super) fn writes(&self) -> bool {
        !matches!(
            self,
            Self::SourceModels(_)
                | Self::GroupReferences(_)
                | Self::Operation(_)
                | Self::Identities(_)
                | Self::Identity(_)
                | Self::NativeTokens { .. }
                | Self::Usage(_)
        )
    }
    pub(super) fn budget(&self) -> Duration {
        let seconds = match self {
            Self::Usage(_) => 15,
            Self::CreateSource(s) if !matches!(&s.selection, ModelSelection::Manual(_)) => 30,
            Self::DiscoverModels { .. }
            | Self::Retry(_)
            | Self::CheckGroup { .. }
            | Self::NativeTokens { .. }
            | Self::RegisterToken { .. }
            | Self::RevealToken { .. } => 30,
            _ => 10,
        };
        Duration::from_secs(seconds)
    }
}
body!(VersionOnly {});
body!(OperationRevision {
    operation_id: String,
    expected_revision: i64
});
body!(Paused {
    operation_id: String,
    expected_revision: i64,
    paused: bool
});
body!(Models { operation_id: String, expected_revision: i64, models: Vec<String> });
body!(ClearError {
    operation_id: String,
    expected_revision: i64,
    expected_runtime_revision: i64
});
body!(UpdatePolicy {
    operation_id: String,
    expected_revision: i64,
    expected_runtime_revision: i64,
    #[serde(deserialize_with = "object")]
    policy: PolicyInput
});
body!(Group {
    name: String,
    newapi_group: String,
    #[serde(rename = "type", deserialize_with = "string_enum")]
    kind: SourceKind
});
body!(CheckGroup {
    expected_revision: i64,
    register: bool
});
body!(Pin { operation_id: String, expected_revision: i64, expected_policy_revision: i64,
    #[serde(deserialize_with="nullable")] membership_id: Option<String> });
body!(Membership {
    source_id: String,
    group_id: String
});
body!(CreateToken { operation_id: String, group_id: String, expected_group_revision: i64, display_name: String,
    #[serde(deserialize_with="nullable")] expires_at: Option<i64> });
body!(Enabled {
    operation_id: String,
    expected_revision: i64,
    enabled: bool
});
body!(Reveal {
    expected_revision: i64
});
body!(Register {
    #[serde(deserialize_with = "string_enum")]
    owner_scope: TokenOwnerScope,
    native_token_id: String,
    display_name: String
});

pub(super) fn decode(route: Route, bytes: &[u8]) -> Result<AdminRequest, AdminFailure> {
    let command = |id: String, command| -> Result<AdminRequest, AdminFailure> {
        Ok(AdminRequest::Control {
            id: route::operation(&id)?,
            command,
        })
    };
    Ok(match route {
        Route::State => return Err(bad_request()), // state只读cache，不构造业务job。
        Route::CreateSource => AdminRequest::CreateSource(source::decode(bytes)?),
        Route::SourceModels(id) => AdminRequest::SourceModels(id),
        Route::SetModels(source_id) => {
            let d: Models = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::SyncSourceModels {
                    source_id,
                    expected_revision: revision(d.expected_revision)?,
                    models: models(d.models)?,
                },
            );
        }
        Route::DiscoverModels(source) => {
            let d: OperationRevision = json(bytes)?;
            AdminRequest::DiscoverModels {
                id: route::operation(&d.operation_id)?,
                source,
                revision: revision(d.expected_revision)?,
            }
        }
        Route::SetSourcePaused(source_id) => {
            let d: Paused = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::SetSourcePaused {
                    source_id,
                    expected_revision: revision(d.expected_revision)?,
                    paused: d.paused,
                },
            );
        }
        Route::RetireSource(source_id) => {
            let d: OperationRevision = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::RetireSource {
                    source_id,
                    expected_revision: revision(d.expected_revision)?,
                },
            );
        }
        Route::ClearVolcanoError(source_id) => {
            let d: ClearError = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::ClearVolcanoError {
                    source_id,
                    expected_revision: revision(d.expected_revision)?,
                    expected_runtime_revision: revision(d.expected_runtime_revision)?,
                },
            );
        }
        Route::UpdateVolcanoPolicy(source_id) => {
            let d: UpdatePolicy = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::UpdateVolcanoPolicy {
                    source_id,
                    expected_revision: revision(d.expected_revision)?,
                    expected_runtime_revision: safe(d.expected_runtime_revision, 0)?,
                    policy: d.policy.validate()?,
                },
            );
        }
        Route::CreateGroup => {
            let d: Group = json(bytes)?;
            if !crate::catalog::types::valid_group(&d.newapi_group) {
                return Err(invalid_input());
            }
            AdminRequest::CreateGroup(GroupDraft {
                name: name(d.name)?,
                newapi_group: d.newapi_group,
                kind: d.kind,
            })
        }
        Route::CheckGroup(group) => {
            let d: CheckGroup = json(bytes)?;
            AdminRequest::CheckGroup {
                group,
                revision: revision(d.expected_revision)?,
                register: d.register,
            }
        }
        Route::GroupReferences(id) => AdminRequest::GroupReferences(id),
        Route::SetGroupPin(group_id) => {
            let d: Pin = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::SetGroupPin {
                    group_id,
                    expected_revision: revision(d.expected_revision)?,
                    expected_policy_revision: revision(d.expected_policy_revision)?,
                    membership_id: d
                        .membership_id
                        .map(|v| {
                            route::id(&v).and_then(|n| MembershipId::new(n).map_err(Into::into))
                        })
                        .transpose()?,
                },
            );
        }
        Route::PlanMembership => {
            let d: Membership = json(bytes)?;
            AdminRequest::PlanMembership {
                source: SourceId::new(route::id(&d.source_id)?)?,
                group: GroupId::new(route::id(&d.group_id)?)?,
            }
        }
        Route::EnsureMember(membership_id) => {
            let d: OperationRevision = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::EnsureMember {
                    membership_id,
                    expected_revision: revision(d.expected_revision)?,
                    status: StatusDirective::Enabled,
                },
            );
        }
        Route::RetireMember(membership_id) => {
            let d: OperationRevision = json(bytes)?;
            return command(
                d.operation_id,
                ControlCommand::RetireMember {
                    membership_id,
                    expected_revision: revision(d.expected_revision)?,
                },
            );
        }
        Route::Operation(id) => AdminRequest::Operation(id),
        Route::Retry(id) => {
            let _: VersionOnly = json(bytes)?;
            AdminRequest::Retry(id)
        }
        Route::Identities(after) => AdminRequest::Identities(after),
        Route::Identity(id) => AdminRequest::Identity(id),
        Route::CreateToken => {
            let d: CreateToken = json(bytes)?;
            AdminRequest::CreateToken {
                id: route::operation(&d.operation_id)?,
                group: GroupId::new(route::id(&d.group_id)?)?,
                revision: revision(d.expected_group_revision)?,
                name: token_label(d.display_name)?,
                expires: d.expires_at.map(|n| safe(n, 0)).transpose()?,
            } // 当前时刻检查留给原幂等提交之后。
        }
        Route::SetTokenEnabled(token) => {
            let d: Enabled = json(bytes)?;
            AdminRequest::SetTokenEnabled {
                id: route::operation(&d.operation_id)?,
                token,
                revision: revision(d.expected_revision)?,
                enabled: d.enabled,
            }
        }
        Route::DeleteToken(token) => {
            let d: OperationRevision = json(bytes)?;
            AdminRequest::DeleteToken {
                id: route::operation(&d.operation_id)?,
                token,
                revision: revision(d.expected_revision)?,
            }
        }
        Route::RevealToken(token) => {
            let d: Reveal = json(bytes)?;
            AdminRequest::RevealToken {
                token,
                revision: revision(d.expected_revision)?,
            }
        }
        Route::NativeTokens(scope, page) => AdminRequest::NativeTokens { scope, page },
        Route::RegisterToken => {
            let d: Register = json(bytes)?;
            AdminRequest::RegisterToken {
                scope: d.owner_scope,
                native: route::id(&d.native_token_id)?,
                name: token_label(d.display_name)?,
            }
        }
        Route::Usage => AdminRequest::Usage(usage::decode(bytes)?),
    })
}
