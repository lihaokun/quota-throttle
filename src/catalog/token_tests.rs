use super::*;
use crate::catalog::control::{
    ControlCommand, ControlErrorCode as Code, CreateState, OperationId, OperationKind,
    OperationPhase, TargetType,
};
use crate::catalog::execution::ExecutionOutcome;
use crate::catalog::tokens::*;
use crate::newapi::tokens::{
    CreationLookup, NativeTokenView, TokenCreateSpec, TokenOwner, TokenOwnerScope, TokenStatus,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

#[path = "token_migration_tests.rs"]
mod migration;
#[path = "token_service_tests.rs"]
mod service;

const NOW: i64 = 2_000_000_000;
fn owner() -> TokenOwner {
    TokenOwner {
        user_id: 41,
        scope: TokenOwnerScope::Issuer,
    }
}
fn op(n: usize) -> OperationId {
    OperationId::new(format!("token-operation-{n:016}")).unwrap()
}
fn fixture() -> (TempDir, CatalogStore, GroupId) {
    let (dir, mut store) = setup();
    let id = store
        .create_group(group("token-group", SourceKind::Glm))
        .unwrap()
        .group_id;
    store
        .conn
        .execute(
            "UPDATE source_groups SET ready=1 WHERE group_id=?1",
            [id.get()],
        )
        .unwrap();
    (dir, store, id)
}
fn command(group: GroupId, expires: Option<i64>) -> TokenCommand {
    TokenCommand::CreateToken {
        owner: owner(),
        group_id: group,
        expected_group_revision: 1,
        display_name: "本地访问 key".into(),
        expires_at: expires,
    }
}
fn submit(
    store: &mut CatalogStore,
    group: GroupId,
    n: usize,
    expires: Option<i64>,
) -> ManagedTokenId {
    let result = store
        .submit_token_operation(op(n), command(group, expires), NOW)
        .unwrap();
    assert_eq!(result.phase, OperationPhase::Queued);
    ManagedTokenId::new(result.target_id.parse().unwrap()).unwrap()
}
fn prepare(store: &mut CatalogStore, n: usize, now: i64) -> TokenTicket {
    let work = store.token_work(&op(n), now).unwrap();
    store.prepare_token_execution(&work, now).unwrap()
}
fn native(id: i64, group: &str) -> NativeTokenView {
    NativeTokenView {
        schema_version: 1,
        token_id: id.to_string(),
        owner_user_id: "41".into(),
        owner_scope: TokenOwnerScope::Issuer,
        name: String::new(),
        masked_key: "private-never-store-this".into(),
        explicit_group: group.into(),
        effective_group: if group.is_empty() {
            "inherited-group".into()
        } else {
            group.into()
        },
        status: TokenStatus::Enabled,
        created_at: NOW,
        expires_at: None,
        remaining_quota: -2,
        used_quota: 2,
        unlimited_quota: true,
        model_limits_enabled: false,
        model_limits: String::new(),
        allow_ips: None,
        cross_group_retry: false,
    }
}
fn materialized(ticket: &TokenTicket, id: i64, status: TokenStatus) -> NativeTokenView {
    let spec = ticket.create_spec().unwrap();
    let mut view = native(id, &spec.group);
    view.name = spec.correlation_name;
    view.expires_at = spec.expires_at;
    view.status = status;
    view
}
fn reference_spec(
    conn: &Connection,
    operation: &OperationId,
    group: &str,
    expires_at: Option<i64>,
) -> TokenCreateSpec {
    let inst: String = conn
        .query_row(
            "SELECT catalog_instance_id FROM catalog_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let encoded = format!(
        "[\"qt-token-create-v1\",\"{inst}\",\"{}\"]",
        operation.as_str()
    );
    let hash = format!("{:x}", Sha256::digest(encoded.as_bytes()));
    TokenCreateSpec {
        correlation_name: format!("qt-t-{}", &hash[..32]),
        group: group.into(),
        expires_at,
    }
}
fn confirm_create(
    store: &mut CatalogStore,
    n: usize,
    ticket: &TokenTicket,
    id: i64,
    now: i64,
) -> NativeTokenView {
    let view = materialized(ticket, id, TokenStatus::Enabled);
    let work = store.token_work(&op(n), now).unwrap();
    let result = store
        .confirm_token(
            &work,
            TokenObservation::Creation(CreationLookup::One(Box::new(view.clone()))),
            now,
        )
        .unwrap();
    assert_eq!(
        (result.phase, result.total_targets, result.confirmed_targets),
        (OperationPhase::Succeeded, 1, 1)
    );
    view
}
fn imported(store: &mut CatalogStore, id: i64, group: &str) -> (ManagedTokenId, NativeTokenView) {
    let view = native(id, group);
    let row = store
        .register_token(owner(), &view, format!("native-{id}"), NOW)
        .unwrap();
    (row.token_id, view)
}
fn image(conn: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut q = conn
                .prepare(&format!(
                    "SELECT * FROM \"{}\" ORDER BY rowid",
                    name.replace('"', "\"\"")
                ))
                .unwrap();
            let n = q.column_count();
            let rows = q
                .query_map([], |r| (0..n).map(|i| r.get(i)).collect())
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}
fn frozen(conn: &Connection) -> BTreeMap<String, Vec<Vec<rusqlite::types::Value>>> {
    let mut result = image(conn);
    for name in [
        "managed_tokens",
        "token_operation_targets",
        "token_execution_attempts",
    ] {
        result.remove(name);
    }
    result
        .get_mut("control_operations")
        .unwrap()
        .retain(|row| row[2] != rusqlite::types::Value::Text("token".into()));
    result
        .get_mut("sqlite_sequence")
        .unwrap()
        .retain(|row| row[0] != rusqlite::types::Value::Text("managed_tokens".into()));
    result
}
fn code<T>(result: crate::catalog::control::ControlResult<T>) -> Code {
    match result {
        Err(e) => e.code,
        Ok(_) => panic!("expected rejection"),
    }
}
fn reopen(dir: &TempDir, store: CatalogStore) -> CatalogStore {
    drop(store);
    CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap()
}

#[test]
fn t5b_schema_owner_identity_and_private_material() {
    let (_dir, mut store, g) = fixture();
    let id = submit(&mut store, g, 0, None);
    let before = store.token_identity(id).unwrap();
    let work = store.token_work(&op(0), NOW).unwrap();
    assert_eq!(work.token_id(), id);
    assert_eq!(work.owner(), owner());
    assert_eq!(work.native_token_id(), None);
    assert_eq!(work.phase(), OperationPhase::Queued);
    assert_eq!(work.create_state(), CreateState::Never);
    let ticket = prepare(&mut store, 0, NOW);
    assert_eq!(ticket.owner(), owner());
    assert_eq!(ticket.native_token_id(), None);
    assert_eq!(ticket.enabled(), None);
    assert!(!ticket.is_delete());
    assert!(!format!("{ticket:?}").contains("token-group"));
    assert_eq!(code(store.token_reveal_target(id, 1)), Code::NotReady);
    confirm_create(&mut store, 0, &ticket, 91, NOW);
    let view = store.token_identity(id).unwrap();
    let value = serde_json::to_value(&view).unwrap();
    let names = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "create_state",
            "created_at",
            "deleted_at",
            "display_name",
            "explicit_group",
            "group_id",
            "native_token_id",
            "origin",
            "owner_scope",
            "owner_user_id",
            "revision",
            "schema_version",
            "token_id"
        ]
    );
    assert_eq!(before.native_token_id, None);
    assert_eq!(view.native_token_id.as_deref(), Some("91"));
    assert_eq!(store.token_reveal_target(id, 1).unwrap(), (owner(), 91));
    assert!(!format!("{:?}", image(&store.conn)).contains("private-never-store-this"));
    assert!(!serde_json::to_string(&view).unwrap().contains("masked_key"));
    println!("B_TOKEN_SCHEMA_JSON={value}");
    println!(
        "B_OPERATION_SCHEMA_JSON={}",
        serde_json::to_string(&store.operation(&op(0)).unwrap()).unwrap()
    );
    println!(
        "B_TOKEN_ENUM_JSON={}",
        json!({"origin":TokenOrigin::VALUES,"owner_scope":[TokenOwnerScope::Issuer,TokenOwnerScope::LegacyAdmin],"create_state":CreateState::VALUES,"kind":OperationKind::VALUES,"target_type":TargetType::VALUES})
    );
}

#[test]
fn t5b_replay_precedes_expiry_revision_and_shared_namespace_conflict() {
    let (_dir, mut store, g) = fixture();
    let id = submit(&mut store, g, 0, Some(NOW + 1));
    let initial = image(&store.conn);
    let result = store
        .submit_token_operation(op(0), command(g, Some(NOW + 1)), NOW + 500)
        .unwrap();
    assert_eq!(result.target_id, id.get().to_string());
    assert_eq!(image(&store.conn), initial);
    assert_eq!(
        code(store.submit_token_operation(op(0), command(g, None), NOW)),
        Code::IdempotencyConflict
    );
    let s = store
        .create_source(source("fake-token-isolation", SourceKind::Glm))
        .unwrap();
    let original = store
        .submit_operation(
            op(1),
            ControlCommand::SetSourcePaused {
                source_id: s.source_id,
                expected_revision: 1,
                paused: true,
            },
            NOW,
        )
        .unwrap();
    assert_eq!(
        code(store.submit_token_operation(op(1), command(g, None), NOW)),
        Code::IdempotencyConflict
    );
    assert_eq!(store.operation(&op(1)).unwrap(), original);
    let stable = image(&store.conn);
    assert_eq!(
        code(store.submit_operation(
            op(0),
            ControlCommand::SetSourcePaused {
                source_id: s.source_id,
                expected_revision: 2,
                paused: false
            },
            NOW
        )),
        Code::IdempotencyConflict
    );
    assert_eq!(image(&store.conn), stable);
}

#[test]
fn t5b_invalid_create_and_owner_scope_changes_are_atomic() {
    let (_dir, mut store, g) = fixture();
    for (scope, revision, label, expiry) in [
        (TokenOwnerScope::LegacyAdmin, 1, "ok", None),
        (TokenOwnerScope::Issuer, 0, "ok", None),
        (TokenOwnerScope::Issuer, 2, "ok", None),
        (TokenOwnerScope::Issuer, 1, "", None),
        (TokenOwnerScope::Issuer, 1, "a\nb", None),
        (TokenOwnerScope::Issuer, 1, "ok", Some(NOW)),
    ] {
        let before = image(&store.conn);
        assert!(store
            .submit_token_operation(
                op(0),
                TokenCommand::CreateToken {
                    owner: TokenOwner { scope, ..owner() },
                    group_id: g,
                    expected_group_revision: revision,
                    display_name: label.into(),
                    expires_at: expiry
                },
                NOW
            )
            .is_err());
        assert_eq!(image(&store.conn), before);
    }
    store
        .conn
        .execute("UPDATE source_groups SET ready=0", [])
        .unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.submit_token_operation(op(0), command(g, None), NOW)),
        Code::NotReady
    );
    assert_eq!(image(&store.conn), before);
    imported(&mut store, 91, "");
    let mut other = native(92, "");
    other.owner_scope = TokenOwnerScope::LegacyAdmin;
    assert_eq!(
        code(store.register_token(
            TokenOwner {
                scope: TokenOwnerScope::LegacyAdmin,
                ..owner()
            },
            &other,
            "other".into(),
            NOW
        )),
        Code::BindingDrift
    );
}

#[test]
fn t5b_every_payload_field_participates_in_idempotency() {
    let (_dir, mut store, g) = fixture();
    let other_group = store
        .create_group(group("other-token-group", SourceKind::Glm))
        .unwrap()
        .group_id;
    submit(&mut store, g, 0, None);
    let make = |own, group_id, expected_group_revision, name: &str, expires_at| {
        TokenCommand::CreateToken {
            owner: own,
            group_id,
            expected_group_revision,
            display_name: name.into(),
            expires_at,
        }
    };
    let before = image(&store.conn);
    for change in [
        make(
            TokenOwner {
                user_id: 42,
                ..owner()
            },
            g,
            1,
            "本地访问 key",
            None,
        ),
        make(
            TokenOwner {
                scope: TokenOwnerScope::LegacyAdmin,
                ..owner()
            },
            g,
            1,
            "本地访问 key",
            None,
        ),
        make(owner(), other_group, 1, "本地访问 key", None),
        make(owner(), g, 2, "本地访问 key", None),
        make(owner(), g, 1, "different label", None),
        make(owner(), g, 1, "本地访问 key", Some(NOW + 99)),
    ] {
        assert_eq!(
            code(store.submit_token_operation(op(0), change, NOW)),
            Code::IdempotencyConflict
        );
        assert_eq!(image(&store.conn), before);
    }
    let (a, _) = imported(&mut store, 91, "");
    let (b, _) = imported(&mut store, 92, "");
    store
        .submit_token_operation(
            op(100),
            TokenCommand::SetTokenEnabled {
                token_id: a,
                expected_revision: 1,
                enabled: true,
            },
            NOW,
        )
        .unwrap();
    for change in [
        TokenCommand::SetTokenEnabled {
            token_id: a,
            expected_revision: 1,
            enabled: false,
        },
        TokenCommand::SetTokenEnabled {
            token_id: b,
            expected_revision: 1,
            enabled: true,
        },
        TokenCommand::SetTokenEnabled {
            token_id: a,
            expected_revision: 2,
            enabled: true,
        },
        TokenCommand::DeleteToken {
            token_id: a,
            expected_revision: 1,
        },
    ] {
        assert_eq!(
            code(store.submit_token_operation(op(100), change, NOW)),
            Code::IdempotencyConflict
        );
    }
    store
        .submit_token_operation(
            op(200),
            TokenCommand::DeleteToken {
                token_id: b,
                expected_revision: 1,
            },
            NOW,
        )
        .unwrap();
    for change in [
        TokenCommand::DeleteToken {
            token_id: a,
            expected_revision: 1,
        },
        TokenCommand::DeleteToken {
            token_id: b,
            expected_revision: 2,
        },
    ] {
        assert_eq!(
            code(store.submit_token_operation(op(200), change, NOW)),
            Code::IdempotencyConflict
        );
    }
}

#[test]
fn t5b_creation_missing_ambiguity_and_ack_never_retry_or_confirm() {
    for outcome in [
        ExecutionOutcome::Uncertain,
        ExecutionOutcome::Responded(None),
        ExecutionOutcome::Responded(Some(Code::RemoteRejected)),
    ] {
        let (dir, mut store, g) = fixture();
        let id = submit(&mut store, g, 0, None);
        let ticket = prepare(&mut store, 0, NOW);
        store.settle_token_execution(&ticket, outcome, NOW).unwrap();
        assert_eq!(
            store.operation(&op(0)).unwrap().phase,
            if outcome == ExecutionOutcome::Responded(None) {
                OperationPhase::Waiting
            } else {
                OperationPhase::Unknown
            }
        );
        for lookup in [CreationLookup::Missing, CreationLookup::Ambiguous] {
            let work = store.token_work(&op(0), NOW).unwrap();
            let result = store
                .confirm_token(&work, TokenObservation::Creation(lookup), NOW)
                .unwrap();
            assert_eq!(result.phase, OperationPhase::Unknown);
            assert_eq!(result.confirmed_targets, 0);
            assert_eq!(
                code(store.confirm_token(
                    &work,
                    TokenObservation::Creation(CreationLookup::Missing),
                    NOW
                )),
                Code::RevisionConflict
            );
        }
        store = reopen(&dir, store);
        store.retry_operation(&op(0), NOW + 1).unwrap();
        let work = store.token_work(&op(0), NOW + 1).unwrap();
        assert_eq!(
            code(store.prepare_token_execution(&work, NOW + 1)),
            Code::NotReady
        );
        assert_eq!(store.token_identity(id).unwrap().native_token_id, None);
        assert_eq!(
            store
                .conn
                .query_row("SELECT count(*) FROM token_execution_attempts", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
    }
}

#[test]
fn t5b_not_sent_retry_is_safe_but_late_not_sent_cannot_erase_binding() {
    let (_dir, mut store, g) = fixture();
    let id = submit(&mut store, g, 0, None);
    let t = prepare(&mut store, 0, NOW);
    store
        .settle_token_execution(&t, ExecutionOutcome::NotSent(Code::RateLimited), NOW)
        .unwrap();
    assert_eq!(
        store.token_identity(id).unwrap().create_state,
        CreateState::Never
    );
    store.retry_operation(&op(0), NOW).unwrap();
    let next = prepare(&mut store, 0, NOW);
    confirm_create(&mut store, 0, &next, 91, NOW);
    let before = image(&store.conn);
    assert_eq!(
        code(store.settle_token_execution(&next, ExecutionOutcome::NotSent(Code::AuthFailed), NOW)),
        Code::RevisionConflict
    );
    assert_eq!(image(&store.conn), before);
    store
        .settle_token_execution(&next, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    let stable = image(&store.conn);
    store
        .settle_token_execution(&next, ExecutionOutcome::Responded(None), NOW + 1)
        .unwrap();
    assert_eq!(image(&store.conn), stable);
}

#[test]
fn t5b_confirmed_prepared_create_and_delete_survive_reopen() {
    let (dir, mut store, g) = fixture();
    let id = submit(&mut store, g, 0, None);
    let t = prepare(&mut store, 0, NOW);
    confirm_create(&mut store, 0, &t, 91, NOW);
    let receipt = store.operation(&op(0)).unwrap();
    store = reopen(&dir, store);
    assert_eq!(store.operation(&op(0)).unwrap(), receipt);
    store
        .conn
        .execute("UPDATE source_groups SET ready=0,revision=revision+1", [])
        .unwrap();
    store = reopen(&dir, store);
    assert_eq!(store.operation(&op(0)).unwrap(), receipt);
    store
        .submit_token_operation(
            op(1),
            TokenCommand::DeleteToken {
                token_id: id,
                expected_revision: 1,
            },
            NOW,
        )
        .unwrap();
    let deletion = prepare(&mut store, 1, NOW);
    assert!(deletion.is_delete());
    assert_eq!(deletion.native_token_id(), Some(91));
    let work = store.token_work(&op(1), NOW).unwrap();
    assert_eq!(
        store
            .confirm_token(&work, TokenObservation::Existing(None), NOW)
            .unwrap()
            .phase,
        OperationPhase::Succeeded
    );
    let stable = image(&store.conn);
    store = reopen(&dir, store);
    assert_eq!(image(&store.conn), stable);
    assert_eq!(store.token_identity(id).unwrap().deleted_at, Some(NOW));
    // 即使原创建HTTP尚无终止回执，已物化后的下一项写也不能被prepared唯一槽卡住。
    let (_live_dir, mut live, group) = fixture();
    let token = submit(&mut live, group, 0, None);
    let creation = prepare(&mut live, 0, NOW);
    confirm_create(&mut live, 0, &creation, 91, NOW);
    live.submit_token_operation(
        op(1),
        TokenCommand::SetTokenEnabled {
            token_id: token,
            expected_revision: 1,
            enabled: false,
        },
        NOW,
    )
    .unwrap();
    let status = prepare(&mut live, 1, NOW);
    assert_eq!(status.enabled(), Some(false));
    assert!(live.token_send_allowed(&status, NOW).unwrap());
}

#[test]
fn t5b_unsettled_status_cannot_be_washed_by_get_retry_or_new_command() {
    let (dir, mut store, _) = fixture();
    let (id, mut view) = imported(&mut store, 91, "");
    store
        .submit_token_operation(
            op(0),
            TokenCommand::SetTokenEnabled {
                token_id: id,
                expected_revision: 1,
                enabled: false,
            },
            NOW,
        )
        .unwrap();
    let ticket = prepare(&mut store, 0, NOW);
    store = reopen(&dir, store);
    view.status = TokenStatus::Disabled;
    let work = store.token_work(&op(0), NOW).unwrap();
    let result = store
        .confirm_token(&work, TokenObservation::Existing(Some(view.clone())), NOW)
        .unwrap();
    assert_eq!(result.phase, OperationPhase::Unknown);
    assert_eq!(
        code(store.retry_operation(&op(0), NOW)),
        Code::TransportUnknown
    );
    assert_eq!(
        code(store.submit_token_operation(
            op(1),
            TokenCommand::DeleteToken {
                token_id: id,
                expected_revision: 2
            },
            NOW
        )),
        Code::NotReady
    );
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    let work = store.token_work(&op(0), NOW).unwrap();
    assert_eq!(
        store
            .confirm_token(&work, TokenObservation::Existing(Some(view)), NOW)
            .unwrap()
            .phase,
        OperationPhase::Succeeded
    );
}

#[test]
fn t5b_old_failed_retry_cannot_block_newer_success() {
    let (_dir, mut store, _) = fixture();
    let (id, view) = imported(&mut store, 91, "");
    store
        .submit_token_operation(
            op(0),
            TokenCommand::SetTokenEnabled {
                token_id: id,
                expected_revision: 1,
                enabled: false,
            },
            NOW,
        )
        .unwrap();
    let ticket = prepare(&mut store, 0, NOW);
    store
        .settle_token_execution(
            &ticket,
            ExecutionOutcome::Responded(Some(Code::RemoteRejected)),
            NOW,
        )
        .unwrap();
    store
        .submit_token_operation(
            op(1),
            TokenCommand::SetTokenEnabled {
                token_id: id,
                expected_revision: 2,
                enabled: true,
            },
            NOW,
        )
        .unwrap();
    let work = store.token_work(&op(1), NOW).unwrap();
    store
        .confirm_token(&work, TokenObservation::Existing(Some(view)), NOW)
        .unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.retry_operation(&op(0), NOW)),
        Code::RevisionConflict
    );
    assert_eq!(image(&store.conn), before);
    store
        .submit_token_operation(
            op(2),
            TokenCommand::DeleteToken {
                token_id: id,
                expected_revision: 3,
            },
            NOW,
        )
        .unwrap();
}

#[test]
fn t5b_send_guard_and_read_fences_cover_time_and_evidence_changes() {
    let (_dir, mut store, g) = fixture();
    submit(&mut store, g, 0, Some(NOW + 2));
    let initial = store.token_work(&op(0), NOW).unwrap();
    let ticket = store.prepare_token_execution(&initial, NOW).unwrap();
    assert!(store.token_send_allowed(&ticket, NOW).unwrap());
    assert!(!store.token_send_allowed(&ticket, NOW + 2).unwrap());
    store
        .conn
        .execute("UPDATE source_groups SET ready=0", [])
        .unwrap();
    assert!(!store.token_send_allowed(&ticket, NOW).unwrap());
    store
        .conn
        .execute("UPDATE source_groups SET ready=1,revision=revision+1", [])
        .unwrap();
    assert!(!store.token_send_allowed(&ticket, NOW).unwrap());
    assert_eq!(
        code(store.note_token_problem(&initial, Code::RateLimited, NOW)),
        Code::RevisionConflict
    );
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Uncertain, NOW)
        .unwrap();
    assert!(!store.token_send_allowed(&ticket, NOW).unwrap());
    for age in [-1, 61] {
        let work = store.token_work(&op(0), NOW + 5).unwrap();
        let before = image(&store.conn);
        assert_eq!(
            code(store.note_token_problem(&work, Code::RateLimited, NOW + 5 + age)),
            Code::NotReady
        );
        assert_eq!(image(&store.conn), before);
    }
    let work = store.token_work(&op(0), NOW + 5).unwrap();
    store
        .note_token_problem(&work, Code::RateLimited, NOW + 65)
        .unwrap();
    // phase/attempt保持Applying/prepared，只改变证据代次也必须撤销旧发送资格。
    let (_second_dir, mut second, group) = fixture();
    submit(&mut second, group, 0, None);
    let ticket = prepare(&mut second, 0, NOW);
    let work = second.token_work(&op(0), NOW).unwrap();
    second
        .note_token_problem(&work, Code::RateLimited, NOW)
        .unwrap();
    assert!(!second.token_send_allowed(&ticket, NOW).unwrap());
}

#[test]
fn t5b_read_diagnostics_throttle_candidates_and_consume_work_once() {
    let (_dir, mut store, g) = fixture();
    submit(&mut store, g, 0, None);
    let ticket = prepare(&mut store, 0, NOW);
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Uncertain, NOW)
        .unwrap();
    assert!(store.pending_token_operations(NOW + 29).unwrap().is_empty());
    assert_eq!(
        store.pending_token_operations(NOW + 30).unwrap(),
        vec![op(0)]
    );
    let work = store.token_work(&op(0), NOW + 30).unwrap();
    store
        .confirm_token(
            &work,
            TokenObservation::Creation(CreationLookup::Missing),
            NOW + 30,
        )
        .unwrap();
    assert!(store.pending_token_operations(NOW + 59).unwrap().is_empty());
    assert_eq!(
        store.pending_token_operations(NOW + 60).unwrap(),
        vec![op(0)]
    );
    let before = image(&store.conn);
    assert_eq!(
        code(store.note_token_problem(&work, Code::RateLimited, NOW + 30)),
        Code::RevisionConflict
    );
    assert_eq!(image(&store.conn), before);
    let work = store.token_work(&op(0), NOW + 60).unwrap();
    store
        .note_token_problem(&work, Code::TransportUnknown, NOW + 60)
        .unwrap();
    let same_time = store.token_work(&op(0), NOW + 60).unwrap();
    store
        .note_token_problem(&same_time, Code::TransportUnknown, NOW + 60)
        .unwrap();
    assert_eq!(
        code(store.note_token_problem(&same_time, Code::TransportUnknown, NOW + 60)),
        Code::RevisionConflict
    );
}

#[test]
fn t5b_legacy_registration_preserves_empty_auto_unknown_group_and_history() {
    let (_dir, mut store) = super::policy_commands::token_history_fixture();
    store
        .create_group(group("token-group", SourceKind::Glm))
        .unwrap();
    let before = frozen(&store.conn);
    for (i, group) in ["", "auto", "foreign-group", "token-group"]
        .into_iter()
        .enumerate()
    {
        let mut view = native(91 + i as i64, group);
        view.owner_user_id = "1".into();
        view.owner_scope = TokenOwnerScope::LegacyAdmin;
        view.name = String::new();
        view.cross_group_retry = true;
        view.model_limits = "m1,m2".into();
        view.model_limits_enabled = true;
        let own = TokenOwner {
            user_id: 1,
            scope: TokenOwnerScope::LegacyAdmin,
        };
        let row = store
            .register_token(own, &view, "旧管理 key".into(), NOW)
            .unwrap();
        assert_eq!(row.explicit_group, group);
        assert_eq!(row.group_id.is_some(), group == "token-group");
        store
            .submit_token_operation(
                op(i),
                TokenCommand::DeleteToken {
                    token_id: row.token_id,
                    expected_revision: 1,
                },
                NOW,
            )
            .unwrap();
        let work = store.token_work(&op(i), NOW).unwrap();
        store
            .confirm_token(&work, TokenObservation::Existing(None), NOW)
            .unwrap();
        let old = store.token_identity(row.token_id).unwrap();
        assert_eq!(
            store
                .register_token(own, &view, "新名字不重写历史".into(), NOW)
                .unwrap(),
            old
        );
        assert_eq!(
            code(store.token_reveal_target(row.token_id, 2)),
            Code::NotReady
        );
    }
    assert_eq!(frozen(&store.conn), before);
}

#[test]
fn t5b_import_cannot_steal_pending_creation_or_reassign_native_id() {
    let (_dir, mut store, g) = fixture();
    submit(&mut store, g, 0, None);
    let ticket = prepare(&mut store, 0, NOW);
    let view = materialized(&ticket, 91, TokenStatus::Enabled);
    assert_eq!(
        code(store.register_token(owner(), &view, "not a bypass".into(), NOW)),
        Code::NotReady
    );
    confirm_create(&mut store, 0, &ticket, 91, NOW);
    let mut other = view.clone();
    other.owner_user_id = "42".into();
    assert_eq!(
        code(store.register_token(
            TokenOwner {
                user_id: 42,
                ..owner()
            },
            &other,
            "other".into(),
            NOW
        )),
        Code::BindingDrift
    );
}

#[test]
fn t5b_malformed_or_wrong_scope_read_never_partially_binds() {
    for fault in 0..8 {
        let (_dir, mut store, g) = fixture();
        let id = submit(&mut store, g, 0, None);
        let ticket = prepare(&mut store, 0, NOW);
        let mut v = materialized(&ticket, 91, TokenStatus::Enabled);
        match fault {
            0 => v.owner_user_id = "42".into(),
            1 => v.name.make_ascii_uppercase(),
            2 => v.explicit_group = "other".into(),
            3 => v.expires_at = Some(NOW + 5),
            4 => v.cross_group_retry = true,
            5 => v.model_limits = "extra".into(),
            6 => v.allow_ips = Some("127.0.0.1".into()),
            _ => v.schema_version = 2,
        }
        let work = store.token_work(&op(0), NOW).unwrap();
        let result = store
            .confirm_token(
                &work,
                TokenObservation::Creation(CreationLookup::One(Box::new(v))),
                NOW,
            )
            .unwrap();
        assert_ne!(result.phase, OperationPhase::Succeeded);
        assert_eq!(result.confirmed_targets, 0);
        assert_eq!(store.token_identity(id).unwrap().native_token_id, None);
    }
}

#[test]
fn t5b_transaction_failures_and_poison_do_not_publish_half_operations() {
    for table in [
        "managed_tokens",
        "control_operations",
        "token_operation_targets",
    ] {
        let (_dir, mut store, g) = fixture();
        store.conn.execute_batch(&format!("CREATE TRIGGER reject_write BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'fake failure'); END")).unwrap();
        let before = image(&store.conn);
        assert_eq!(
            code(store.submit_token_operation(op(0), command(g, None), NOW)),
            Code::Storage
        );
        assert_eq!(image(&store.conn), before);
        assert!(!store.poisoned);
    }
    let (_dir, mut store, g) = fixture();
    let id = submit(&mut store, g, 0, None);
    let work = store.token_work(&op(0), NOW).unwrap();
    store.conn.execute_batch("CREATE TRIGGER reject_attempt BEFORE INSERT ON token_execution_attempts BEGIN SELECT RAISE(ABORT,'fake failure'); END").unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.prepare_token_execution(&work, NOW)),
        Code::Storage
    );
    assert_eq!(image(&store.conn), before);
    store.poisoned = true;
    assert_eq!(code(store.token_identity(id)), Code::Storage);
    assert_eq!(
        code(store.submit_token_operation(op(1), command(g, None), NOW)),
        Code::Storage
    );
    assert_eq!(image(&store.conn), before);
}

#[test]
fn t5b_settle_confirm_retry_and_recovery_failures_are_atomic() {
    let (_dir, mut store, group) = fixture();
    let id = submit(&mut store, group, 0, None);
    let ticket = prepare(&mut store, 0, NOW);
    store.conn.execute_batch("CREATE TRIGGER fail_settle BEFORE UPDATE ON token_execution_attempts BEGIN SELECT RAISE(ABORT,'fake');END").unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.settle_token_execution(&ticket, ExecutionOutcome::Uncertain, NOW)),
        Code::Storage
    );
    assert_eq!(image(&store.conn), before);
    store.conn.execute("DROP TRIGGER fail_settle", []).unwrap();
    store
        .settle_token_execution(&ticket, ExecutionOutcome::Responded(None), NOW)
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_confirm BEFORE UPDATE ON token_operation_targets BEGIN SELECT RAISE(ABORT,'fake');END").unwrap();
    let work = store.token_work(&op(0), NOW).unwrap();
    let before = image(&store.conn);
    assert_eq!(
        code(store.confirm_token(
            &work,
            TokenObservation::Creation(CreationLookup::One(Box::new(materialized(
                &ticket,
                91,
                TokenStatus::Enabled
            )))),
            NOW
        )),
        Code::Storage
    );
    assert_eq!(image(&store.conn), before);
    store.conn.execute("DROP TRIGGER fail_confirm", []).unwrap();
    confirm_create(&mut store, 0, &ticket, 91, NOW);
    store
        .submit_token_operation(
            op(1),
            TokenCommand::SetTokenEnabled {
                token_id: id,
                expected_revision: 1,
                enabled: false,
            },
            NOW,
        )
        .unwrap();
    let status = prepare(&mut store, 1, NOW);
    store
        .settle_token_execution(&status, ExecutionOutcome::NotSent(Code::AuthFailed), NOW)
        .unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_retry BEFORE UPDATE ON control_operations BEGIN SELECT RAISE(ABORT,'fake');END").unwrap();
    let before = image(&store.conn);
    assert_eq!(code(store.retry_operation(&op(1), NOW)), Code::Storage);
    assert_eq!(image(&store.conn), before);

    let (dir, mut recovering, group) = fixture();
    for n in 0..2 {
        submit(&mut recovering, group, n, None);
        prepare(&mut recovering, n, NOW);
    }
    recovering.conn.execute_batch("CREATE TRIGGER fail_recover BEFORE UPDATE ON token_execution_attempts WHEN NEW.token_id=2 BEGIN SELECT RAISE(ABORT,'fake');END").unwrap();
    let before = image(&recovering.conn);
    drop(recovering);
    assert!(CatalogStore::open(&dir.path().join("catalog.sqlite3")).is_err());
    let conn = Connection::open(dir.path().join("catalog.sqlite3")).unwrap();
    assert_eq!(image(&conn), before);
    conn.execute("DROP TRIGGER fail_recover", []).unwrap();
    drop(conn);
    let recovered = CatalogStore::open(&dir.path().join("catalog.sqlite3")).unwrap();
    for n in 0..2 {
        assert_eq!(
            recovered.operation(&op(n)).unwrap().phase,
            OperationPhase::Unknown
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5b_pb1_replays_and_creation_marker_match_independent_reference(n in 0usize..1000,replays in 0usize..6,expired in any::<bool>()) {
        let (dir,mut store,g)=fixture();let expiry=expired.then_some(NOW+1);let id=submit(&mut store,g,n,expiry);
        let expected=reference_spec(&store.conn,&op(n),"token-group",expiry);let work=store.token_work(&op(n),NOW).unwrap();prop_assert_eq!(work.create_spec().unwrap(),expected);
        let encoded=serde_json::to_vec(&json!(["qt-token-command-v1","create_token",[41,"issuer",g.get().to_string(),1,"本地访问 key",expiry]])).unwrap();let expected_digest=format!("{:x}",Sha256::digest(encoded));let stored:String=store.conn.query_row("SELECT payload_digest FROM control_operations WHERE operation_id=?1",[op(n).as_str()],|r|r.get(0)).unwrap();prop_assert_eq!(stored,expected_digest);
        let before=image(&store.conn);for _ in 0..replays {store=reopen(&dir,store);let v=store.submit_token_operation(op(n),command(g,expiry),NOW+100).unwrap();prop_assert_eq!(v.target_id,id.get().to_string());}prop_assert_eq!(image(&store.conn),before);
    }
    #[test]
    fn t5b_pb2_creation_delivery_reopen_and_expired_lookup_model(deliveries in prop::collection::vec(0u8..3,0..5),status in 0u8..4,late in any::<bool>()) {
        let (dir,mut store,g)=fixture();let expiry=late.then_some(NOW+2);let id=submit(&mut store,g,0,expiry);let mut attempts=0;let mut sent=false;let mut final_ticket=None;
        for delivery in deliveries.into_iter().chain([2]) {
            if sent {break;}let ticket=prepare(&mut store,0,NOW);attempts+=1;
            if delivery==0 {store.settle_token_execution(&ticket,ExecutionOutcome::NotSent(Code::RateLimited),NOW).unwrap();store=reopen(&dir,store);store.retry_operation(&op(0),NOW).unwrap();}
            else {sent=true;if delivery==1 {store.settle_token_execution(&ticket,ExecutionOutcome::Responded(None),NOW).unwrap();}store=reopen(&dir,store);final_ticket=Some(ticket);}
        }
        let ticket=final_ticket.unwrap();let now=NOW+5;let work=store.token_work(&op(0),now).unwrap();store.confirm_token(&work,TokenObservation::Creation(CreationLookup::Missing),now).unwrap();let work=store.token_work(&op(0),now).unwrap();prop_assert_eq!(code(store.prepare_token_execution(&work,now)),Code::NotReady);
        let native_status=[TokenStatus::Enabled,TokenStatus::Disabled,TokenStatus::Expired,TokenStatus::Exhausted][status as usize];let expected=reference_spec(&store.conn,&op(0),"token-group",expiry);prop_assert_eq!(ticket.create_spec().unwrap(),expected.clone());let mut view=native(91,"token-group");view.name=expected.correlation_name;view.expires_at=expiry;view.status=native_status;let work=store.token_work(&op(0),now).unwrap();prop_assert_eq!(store.confirm_token(&work,TokenObservation::Creation(CreationLookup::One(Box::new(view))),now).unwrap().phase,OperationPhase::Succeeded);
        store=reopen(&dir,store);prop_assert_eq!(store.token_identity(id).unwrap().native_token_id,Some("91".into()));prop_assert_eq!(store.conn.query_row("SELECT count(*) FROM token_execution_attempts",[],|r|r.get::<_,i64>(0)).unwrap(),attempts);
    }
    #[test]
    fn t5b_pb3_status_sequences_fence_old_reads_and_preserve_receipts(events in prop::collection::vec((any::<bool>(),any::<bool>(),any::<bool>()),1..8)) {
        let (dir,mut store,_)=fixture();let (id,mut native)=imported(&mut store,91,"");let mut revision=1;let mut receipts=Vec::new();
        for (i,(enabled,uncertain,restart)) in events.into_iter().enumerate() {
            store.submit_token_operation(op(i),TokenCommand::SetTokenEnabled{token_id:id,expected_revision:revision,enabled},NOW+i as i64).unwrap();revision+=1;
            let old=store.token_work(&op(i),NOW+i as i64).unwrap();let ticket=store.prepare_token_execution(&old,NOW+i as i64).unwrap();native.status=if enabled{TokenStatus::Enabled}else{TokenStatus::Disabled};
            prop_assert_eq!(code(store.confirm_token(&old,TokenObservation::Existing(Some(native.clone())),NOW+i as i64)),Code::RevisionConflict);
            if uncertain {store.settle_token_execution(&ticket,ExecutionOutcome::Uncertain,NOW+i as i64).unwrap();let work=store.token_work(&op(i),NOW+i as i64).unwrap();prop_assert_eq!(store.confirm_token(&work,TokenObservation::Existing(Some(native.clone())),NOW+i as i64).unwrap().phase,OperationPhase::Unknown);}
            if restart {store=reopen(&dir,store);}store.settle_token_execution(&ticket,ExecutionOutcome::Responded(None),NOW+i as i64).unwrap();let work=store.token_work(&op(i),NOW+i as i64).unwrap();let done=store.confirm_token(&work,TokenObservation::Existing(Some(native.clone())),NOW+i as i64).unwrap();prop_assert_eq!(done.phase,OperationPhase::Succeeded);receipts.push(done);
            for (j,receipt) in receipts.iter().enumerate() {prop_assert_eq!(store.operation(&op(j)).unwrap(),receipt.clone());}prop_assert_eq!(store.token_identity(id).unwrap().revision,revision);
        }
    }
    #[test]
    fn t5b_pb4_import_delete_keeps_identity_and_unrelated_history(ids in prop::collection::btree_set(1i64..10000,1..12),restart in any::<bool>()) {
        let (dir,mut store)=super::policy_commands::token_history_fixture();let before=frozen(&store.conn);let mut expected=BTreeMap::new();let mut ids=ids;ids.insert(77); // 与历史账本token ID交叉，不能只验证无关空历史。
        for (i,native_id) in ids.into_iter().enumerate() {
            let (id,view)=imported(&mut store,native_id,if i%2==0{""}else{"foreign"});expected.insert(id,native_id);store.submit_token_operation(op(i),TokenCommand::DeleteToken{token_id:id,expected_revision:1},NOW).unwrap();let t=prepare(&mut store,i,NOW);store.settle_token_execution(&t,ExecutionOutcome::Uncertain,NOW).unwrap();let work=store.token_work(&op(i),NOW).unwrap();store.confirm_token(&work,TokenObservation::Existing(None),NOW).unwrap();let after=store.register_token(owner(),&view,"must not rename".into(),NOW).unwrap();prop_assert_eq!(after.token_id,id);prop_assert_eq!(after.deleted_at,Some(NOW));
        }
        if restart {store=reopen(&dir,store);}for (id,native) in expected {prop_assert_eq!(store.token_identity(id).unwrap().native_token_id,Some(native.to_string()));}prop_assert_eq!(frozen(&store.conn),before);
    }
}
