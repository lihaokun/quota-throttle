//! 管理面的窄只读投影；复用同目录连接，不读取来源凭据表。
use super::control::ControlResult;
use super::types::{error, fingerprint, ErrorKind};
use super::{CatalogStore, SecretString, SourceId, SourceSettings};
use rusqlite::OptionalExtension;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct SourceModelsView {
    pub schema_version: i64,
    pub source_id: SourceId,
    pub source_revision: i64,
    pub models: Vec<String>,
}
impl CatalogStore {
    pub(crate) fn source_models(&mut self, id: SourceId) -> ControlResult<SourceModelsView> {
        self.usable()?;
        let tx = self.conn.transaction()?;
        let (source, settings, _) = super::policy::source_record(&tx, id)?;
        let upstream = match settings {
            SourceSettings::Glm { upstream, .. } | SourceSettings::Volcano { upstream, .. } => {
                upstream
            }
        };
        Ok(SourceModelsView {
            schema_version: 1,
            source_id: source.source_id,
            source_revision: source.config_revision,
            models: upstream.models,
        })
    }
    pub(crate) fn source_for_credential(
        &self,
        credential: &SecretString,
    ) -> super::types::Result<Option<SourceId>> {
        self.usable()?;
        let value: Option<i64> = self
            .conn
            .query_row(
                "SELECT source_id FROM sources WHERE credential_fingerprint=?1",
                [fingerprint(credential)],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|n| SourceId::new(n).map_err(|_| error(ErrorKind::CorruptData)))
            .transpose()
    }
}
