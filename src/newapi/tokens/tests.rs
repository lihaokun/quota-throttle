use super::*;
use crate::newapi::transport::tests::{Fake, Reply};
use crate::newapi::transport::{ManagementBudget, ManagementError};
use crate::newapi::NewApiClient;
use proptest::prelude::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const KEY: &str = "PrivateNativeAccessKey0123456789abcdef0123456789ab";
const TAG: &str = "qt-t-0123456789abcdef0123456789abcdef";
fn owner() -> TokenOwner {
    TokenOwner {
        user_id: 41,
        scope: TokenOwnerScope::Issuer,
    }
}
fn spec() -> TokenCreateSpec {
    TokenCreateSpec {
        correlation_name: TAG.into(),
        group: "sgm-a".into(),
        expires_at: None,
    }
}
fn profile_value(id: i64, role: i64) -> Value {
    json!({"id":id,"role":role,"status":1,"group":"default","quota":10000})
}
fn profile() -> TokenOwnerProfile {
    TokenOwnerProfile {
        user_id: 41,
        role: 1,
        account_group: "default".into(),
        quota: 10000,
    }
}
fn token(id: i64) -> Value {
    json!({"id":id,"user_id":41,"name":TAG,"key":KEY,"group":"sgm-a","status":1,
        "created_time":100,"expired_time":-1,"remain_quota":0,"used_quota":12,
        "unlimited_quota":true,"model_limits_enabled":false,"model_limits":"","allow_ips":"",
        "cross_group_retry":false,"unknown_private":"DO_NOT_EXPOSE","accessed_time":150})
}
fn ok(data: Value) -> Reply {
    Reply::json(json!({"success":true,"data":data}))
}
fn page(items: Vec<Value>, total: usize, number: usize, size: usize) -> Value {
    json!({"items":items,"total":total,"page":number,"page_size":size})
}
fn missing() -> Reply {
    Reply::json(json!({"success":false,"message":"record not found"}))
}
fn groups() -> Reply {
    ok(json!({"sgm-a":{"ratio":1},"v_2":{"ratio":0},"auto":{"ratio":"自动"}}))
}
fn client(fake: &Fake) -> NewApiClient {
    NewApiClient::new(&fake.config(true)).unwrap()
}
async fn drive(future: impl std::future::Future<Output = ()> + Send + 'static) {
    let task = tokio::spawn(future);
    for _ in 0..200_000 {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        if task.is_finished() {
            task.await.unwrap();
            return;
        }
        tokio::time::advance(Duration::from_millis(10)).await;
    }
    task.abort();
    panic!("bounded token scenario did not finish");
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
}
fn writes(fake: &Fake) -> Vec<(String, String, Value)> {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.path.starts_with("/api/token/") && c.method != "GET")
        .map(|c| (c.method.clone(), c.path.clone(), c.body.clone()))
        .collect()
}
fn code<T>(value: Result<T>) -> ManagementError {
    value.err().expect("must fail")
}

#[test]
fn t5a_view_schema_and_private_material() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(RevealedToken, std::fmt::Debug);
    assert_not_impl!(RevealedToken, serde::Serialize);
    let view = parse_token(&token(i64::MAX), &profile(), owner()).unwrap();
    let json = serde_json::to_value(view).unwrap();
    assert_eq!(json.as_object().unwrap().len(), 18);
    assert!(!json.to_string().contains(KEY));
    assert!(!json.to_string().contains("DO_NOT_EXPOSE"));
    assert_eq!(json["token_id"], i64::MAX.to_string());
    println!("TOKEN_SCHEMA_JSON={json}");
    println!(
        "TOKEN_ENUM_JSON={}",
        json!({"owner_scope":[TokenOwnerScope::Issuer,TokenOwnerScope::LegacyAdmin],"status":[TokenStatus::Enabled,TokenStatus::Disabled,TokenStatus::Expired,TokenStatus::Exhausted]})
    );
}
#[test]
fn t5a_bad_fields_are_unknown_not_defaults_and_negative_quota_is_valid() {
    for field in [
        "id",
        "user_id",
        "name",
        "key",
        "group",
        "status",
        "created_time",
        "expired_time",
        "remain_quota",
        "used_quota",
        "unlimited_quota",
        "model_limits_enabled",
        "model_limits",
        "allow_ips",
        "cross_group_retry",
    ] {
        let mut raw = token(1);
        raw.as_object_mut().unwrap().remove(field);
        assert_eq!(
            code(parse_token(&raw, &profile(), owner())).kind,
            Code::InvalidResponse,
            "{field}"
        );
    }
    for (field, value) in [
        ("id", json!(0)),
        ("id", json!(u64::MAX)),
        ("user_id", json!(42)),
        ("status", json!(5)),
        ("expired_time", json!(-2)),
        ("used_quota", json!(-1)),
        ("remain_quota", json!(JSON_SAFE + 1)),
        ("key", json!("")),
        ("key", json!("含密钥")),
        ("allow_ips", json!(false)),
    ] {
        let mut raw = token(1);
        raw[field] = value;
        assert!(parse_token(&raw, &profile(), owner()).is_err(), "{field}");
    }
    let mut raw = token(1);
    raw["remain_quota"] = json!(-17);
    raw["status"] = json!(4);
    raw["allow_ips"] = Value::Null;
    let v = parse_token(&raw, &profile(), owner()).unwrap();
    assert_eq!(v.remaining_quota, -17);
    assert_eq!(v.status, TokenStatus::Exhausted);
    assert_eq!(v.allow_ips, None);
}
#[test]
fn t5a_bad_pages_are_not_empty_success() {
    for p in [
        json!({}),
        page(vec![token(1)], 101, 1, 100),
        page(vec![token(1)], 1, 2, 100),
        page(vec![token(1)], 1, 1, 99),
        page(vec![], 10001, 1, 100),
        page(vec![token(1), token(1)], 2, 1, 100),
        page(vec![token(1), token(2)], 2, 1, 100),
    ] {
        assert!(parse_page(&p, 1, 100, &profile(), owner()).is_err());
    }
    assert_eq!(
        parse_page(&page(vec![], 0, 1, 100), 1, 100, &profile(), owner())
            .unwrap()
            .total,
        0
    );
}

#[tokio::test(start_paused = true)]
async fn t5a_invalid_inputs_send_nothing() {
    drive(async {
        assert_eq!(
            code(ManagementRequest::token_lookup(1).unwrap().for_user(0)).kind,
            Code::InvalidInput
        );
        let fake = Fake::start(|_| panic!("invalid input must not send")).await;
        let c = client(&fake);
        let api = c.managed_tokens(owner());
        for p in [0, 101, usize::MAX] {
            assert_eq!(code(api.list_page(p).await).kind, Code::InvalidInput);
        }
        for id in [0, -1] {
            assert_eq!(code(api.get(id).await).kind, Code::InvalidInput);
            assert_eq!(
                code(api.set_enabled(id, false).await).kind,
                Code::InvalidInput
            );
            assert_eq!(code(api.delete(id).await).kind, Code::InvalidInput);
            assert_eq!(code(api.reveal(id).await).kind, Code::InvalidInput);
        }
        assert_eq!(
            code(
                c.managed_tokens(TokenOwner {
                    user_id: 0,
                    ..owner()
                })
                .profile()
                .await
            )
            .kind,
            Code::InvalidInput
        );
        let mut draft = spec();
        draft.expires_at = Some(0);
        assert_eq!(code(api.create(&draft).await).kind, Code::InvalidInput);
        assert_eq!(
            code(
                c.managed_tokens(TokenOwner {
                    scope: TokenOwnerScope::LegacyAdmin,
                    ..owner()
                })
                .create(&spec())
                .await
            )
            .kind,
            Code::InvalidInput
        );
        assert!(fake.calls.lock().unwrap().is_empty());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_signer_readiness_never_falls_back_to_admin() {
    drive(async {
        for mode in 0..6 {
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/self" => {
                    let mut p = profile_value(41, 1);
                    match mode {
                        0 => p["id"] = json!(42),
                        1 => p["role"] = json!(10),
                        2 => p["status"] = json!(2),
                        3 => p["quota"] = json!(0),
                        4 => p["quota"] = json!(-1),
                        _ => {}
                    };
                    ok(p)
                }
                "/api/user/self/groups" => ok(json!({"default":{"ratio":1}})),
                _ => panic!("unready owner must not post token"),
            })
            .await;
            let c = client(&fake);
            let err = code(c.managed_tokens(owner()).create(&spec()).await);
            assert_eq!(
                err.kind,
                if mode < 3 {
                    Code::AuthFailed
                } else {
                    Code::RemoteRejected
                }
            );
            assert_eq!(err.delivery, DeliveryEvidence::NotSent);
            assert!(writes(&fake).is_empty());
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_lookup_recovers_expired_creation_and_never_picks_ambiguous_or_casefolded_name() {
    drive(async {
    for mode in 0..6 {
        let fake=Fake::start(move |call|if call.path=="/api/user/self" {let mut p=profile_value(41,1);p["quota"]=json!(0);ok(p)} else {
            assert_eq!(call.path,format!("/api/token/search?keyword={TAG}&p=1&page_size=2"));
            let mut t=token(2);t["expired_time"]=json!(1);t["status"]=json!(3);
            match mode {0=>ok(page(vec![],0,1,2)),1=>ok(page(vec![t],1,1,2)),2=>{let mut b=t.clone();b["id"]=json!(1);ok(page(vec![t,b],2,1,2))},3=>{t["name"]=json!(TAG.to_uppercase());ok(page(vec![t],1,1,2))},4=>{t["group"]=json!("v_2");ok(page(vec![t],1,1,2))},_=>{t["cross_group_retry"]=json!(true);ok(page(vec![t],1,1,2))}}
        }).await;
        let c=client(&fake);let mut s=spec();s.expires_at=Some(1);
        let found=c.managed_tokens(owner()).lookup_created(&s).await;
        match mode {0=>assert_eq!(found.unwrap(),CreationLookup::Missing),1=>assert!(matches!(found.unwrap(),CreationLookup::One(v) if v.status==TokenStatus::Expired)),2=>assert_eq!(found.unwrap(),CreationLookup::Ambiguous),_=>assert_eq!(code(found).kind,Code::InvalidResponse)}
        assert!(writes(&fake).is_empty());
    }
}).await;
}

#[tokio::test(start_paused = true)]
async fn t5a_missing_is_only_the_explicit_token_lookup_case() {
    drive(async {
        for mode in 0..6 {
            let fake = Fake::start(move |call| {
                if call.path == "/api/user/self" {
                    ok(profile_value(41, 1))
                } else {
                    match mode {
                        0 => missing(),
                        1 => Reply::json(
                            json!({"success":false,"message":"record not found","data":null}),
                        ),
                        2 => Reply::json(json!({"success":false,"message":"database unavailable"})),
                        3 => Reply::json(
                            json!({"success":false,"message":"record not found","data":{}}),
                        ),
                        4 => Reply {
                            status: 404,
                            ..missing()
                        },
                        _ => ok(Value::Null),
                    }
                }
            })
            .await;
            let c = client(&fake);
            let result = c.managed_tokens(owner()).get(1).await;
            if mode < 2 {
                assert_eq!(result.unwrap(), None);
            } else {
                assert!(result.is_err());
            }
            let transport = fake.transport(true);
            for path in ["/api/token/1", "/api/channel/1"] {
                let r = ManagementRequest::new(
                    Method::GET,
                    path,
                    None,
                    RequestSemantics::Read,
                    SINGLE_BODY_LIMIT,
                )
                .unwrap();
                assert!(transport.send(r, RequestClass::Control).await.is_err() || mode == 5);
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_legacy_inheritance_status_and_single_delete_preserve_other_fields() {
    drive(async {
        let state = Arc::new(Mutex::new(Some(token(7))));
        state.lock().unwrap().as_mut().unwrap()["group"] = json!("");
        state.lock().unwrap().as_mut().unwrap()["cross_group_retry"] = json!(true);
        let s = state.clone();
        let fake = Fake::start(
            move |call| match (call.method.as_str(), call.path.as_str()) {
                ("GET", "/api/user/self") => ok(profile_value(41, 100)),
                ("GET", "/api/token/?p=1&page_size=100") => ok(page(
                    vec![s.lock().unwrap().as_ref().unwrap().clone()],
                    1,
                    1,
                    100,
                )),
                ("GET", "/api/token/7") => match s.lock().unwrap().as_ref() {
                    Some(t) => ok(t.clone()),
                    None => missing(),
                },
                ("PUT", "/api/token/?status_only=true") => {
                    assert_eq!(call.body, json!({"id":7,"status":2}));
                    let mut state = s.lock().unwrap();
                    state.as_mut().unwrap()["status"] = json!(2);
                    ok(state.as_ref().unwrap().clone())
                }
                ("POST", "/api/token/batch") => {
                    assert_eq!(call.body, json!({"ids":[7]}));
                    ok(json!(i64::from(s.lock().unwrap().take().is_some())))
                }
                _ => panic!("unexpected legacy call"),
            },
        )
        .await;
        let c = client(&fake);
        let api = c.managed_tokens(TokenOwner {
            scope: TokenOwnerScope::LegacyAdmin,
            ..owner()
        });
        let page = api.list_page(1).await.unwrap();
        assert_eq!(page.items[0].effective_group, "default");
        assert_eq!(page.items[0].explicit_group, "");
        assert!(page.items[0].cross_group_retry);
        api.set_enabled(7, false).await.unwrap();
        assert_eq!(
            api.get(7).await.unwrap().unwrap().status,
            TokenStatus::Disabled
        );
        api.delete(7).await.unwrap();
        api.delete(7).await.unwrap();
        assert_eq!(api.get(7).await.unwrap(), None);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_reveal_is_explicit_prefixed_once_and_errors_are_redacted() {
    drive(async {
        for raw in [
            KEY,
            "",
            "sk-badAlreadyPrefixed000000000",
            "masked********0000",
            "bad-token-suffix0000",
            "PRIVATE_RESPONSE\n",
        ] {
            let key = raw.to_string();
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/self" => ok(profile_value(41, 1)),
                "/api/token/1" => ok(token(1)),
                "/api/token/1/key" => ok(json!({"key":key})),
                _ => panic!(),
            })
            .await;
            let c = client(&fake);
            let api = c.managed_tokens(owner());
            let v = api.get(1).await.unwrap().unwrap();
            assert!(!serde_json::to_string(&v).unwrap().contains(KEY));
            assert!(writes(&fake).is_empty());
            let revealed = api.reveal(1).await;
            if raw == KEY {
                assert_eq!(revealed.unwrap().expose(), format!("sk-{KEY}"));
            } else {
                let e = code(revealed);
                assert_eq!(e.kind, Code::InvalidResponse);
                assert!(!format!("{e:?} {e}").contains("PRIVATE_RESPONSE"));
            }
            assert_eq!(writes(&fake).len(), 1);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_write_gate_is_rechecked_after_preflight() {
    drive(async {
        let gate = Arc::new(AtomicBool::new(true));
        let g = gate.clone();
        let fake = Fake::start(move |call| match call.path.as_str() {
            "/api/user/self" => ok(profile_value(41, 1)),
            "/api/token/1" => {
                g.store(false, Ordering::SeqCst);
                ok(token(1))
            }
            _ => panic!("closed gate must not write"),
        })
        .await;
        let c = client(&fake);
        let api = c.managed_tokens(owner());
        let guard = || gate.load(Ordering::SeqCst);
        let e = code(api.fenced(&guard).set_enabled(1, false).await);
        assert_eq!(e.kind, Code::RevisionConflict);
        assert_eq!(e.delivery, DeliveryEvidence::NotSent);
        assert!(writes(&fake).is_empty());
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_401_replay_rechecks_gate_and_keeps_owner_session() {
    drive(async {
        let gate = Arc::new(AtomicBool::new(true));
        let g = gate.clone();
        let fake = Fake::start(move |call| match call.path.as_str() {
            "/api/user/login" => ok(json!({"id":41})),
            "/api/user/self" => ok(profile_value(41, 1)),
            "/api/user/self/groups" => groups(),
            "/api/token/" => {
                g.store(false, Ordering::SeqCst);
                Reply {
                    status: 401,
                    ..Reply::json(json!({"success":false,"message":"expired"}))
                }
            }
            _ => panic!(),
        })
        .await;
        let c = NewApiClient::new(&fake.config(false)).unwrap();
        let api = c.managed_tokens(owner());
        let guard = || gate.load(Ordering::SeqCst);
        let e = code(api.fenced(&guard).create(&spec()).await);
        assert_eq!(e.kind, Code::RevisionConflict);
        assert_eq!(e.delivery, DeliveryEvidence::NotSent);
        assert_eq!(writes(&fake).len(), 1);
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.path == "/api/user/login")
                .count(),
            2
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_two_owner_sessions_share_budget_not_cookie() {
    drive(async {
        let fake = Fake::start(|call| match call.path.as_str() {
            "/api/user/login" => {
                let ordinary = call.body["username"] == "issuer";
                Reply {
                    headers: format!(
                        "Set-Cookie: scope={}; Path=/\r\n",
                        if ordinary { "issuer" } else { "admin" }
                    ),
                    ..ok(json!({"id":if ordinary{41}else{1}}))
                }
            }
            "/api/user/self" => {
                if call.raw.contains("scope=issuer") {
                    assert!(call.raw.to_lowercase().contains("new-api-user: 41"));
                    ok(profile_value(41, 1))
                } else {
                    assert!(call.raw.contains("scope=admin"));
                    assert!(call.raw.to_lowercase().contains("new-api-user: 1"));
                    ok(profile_value(1, 100))
                }
            }
            _ => panic!(),
        })
        .await;
        let budget = ManagementBudget::new();
        let mut a = fake.config(false);
        a.root_username = "issuer".into();
        let mut b = fake.config(false);
        b.root_username = "admin".into();
        let ordinary = NewApiClient::with_budget(&a, budget.clone()).unwrap();
        let admin = NewApiClient::with_budget(&b, budget).unwrap();
        let oa = ordinary.managed_tokens(owner());
        let ob = admin.managed_tokens(TokenOwner {
            user_id: 1,
            scope: TokenOwnerScope::LegacyAdmin,
        });
        let (a, b) = tokio::join!(oa.profile(), ob.profile());
        assert_eq!(a.unwrap().user_id, 41);
        assert_eq!(b.unwrap().user_id, 1);
        oa.profile().await.unwrap();
        ob.profile().await.unwrap();
        let calls = fake.calls.lock().unwrap();
        assert_eq!(calls.len(), 6);
        assert!(
            calls
                .last()
                .unwrap()
                .at
                .duration_since(calls.first().unwrap().at)
                >= Duration::from_secs(4)
        );
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_bound_owner_survives_reauthentication() {
    drive(async {
        for renewed in [41, 42] {
            let logins = Arc::new(AtomicUsize::new(0));
            let l = logins.clone();
            let posts = Arc::new(AtomicUsize::new(0));
            let p = posts.clone();
            let foreign = Arc::new(AtomicUsize::new(0));
            let f = foreign.clone();
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/login" => {
                    let id = if l.fetch_add(1, Ordering::SeqCst) == 0 {
                        41
                    } else {
                        renewed
                    };
                    Reply {
                        headers: format!("Set-Cookie: principal={id}; Path=/\r\n"),
                        ..ok(json!({"id":id}))
                    }
                }
                "/api/user/self" => ok(profile_value(41, 1)),
                "/api/user/self/groups" => groups(),
                "/api/token/" => {
                    let attempt = p.fetch_add(1, Ordering::SeqCst);
                    let actual = if call.raw.contains("principal=42") {
                        42
                    } else {
                        41
                    };
                    let header = call
                        .raw
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("new-api-user"))
                                .map(|(_, v)| v.trim().parse::<i64>().unwrap())
                        })
                        .unwrap();
                    if attempt == 0 || header != actual {
                        Reply {
                            status: 401,
                            ..Reply::json(json!({"success":false}))
                        }
                    } else {
                        if actual != 41 {
                            f.fetch_add(1, Ordering::SeqCst);
                        }
                        Reply::json(json!({"success":true}))
                    }
                }
                _ => panic!("unexpected endpoint"),
            })
            .await;
            let c = NewApiClient::new(&fake.config(false)).unwrap();
            let result = c.managed_tokens(owner()).create(&spec()).await;
            if renewed == 41 {
                result.unwrap();
                assert_eq!(posts.load(Ordering::SeqCst), 2);
            } else {
                let e = code(result);
                assert_eq!(e.kind, Code::AuthFailed);
                assert_eq!(e.delivery, DeliveryEvidence::NotSent);
                assert_eq!(posts.load(Ordering::SeqCst), 1);
            }
            assert_eq!(logins.load(Ordering::SeqCst), 2);
            assert_eq!(foreign.load(Ordering::SeqCst), 0);
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_bound_owner_bearer_header_overrides_static_value() {
    drive(async {
        let fake = Fake::start(|call| {
            let ids = call
                .raw
                .lines()
                .filter_map(|line| {
                    line.split_once(':')
                        .filter(|(k, _)| k.eq_ignore_ascii_case("new-api-user"))
                        .map(|(_, v)| v.trim())
                })
                .collect::<Vec<_>>();
            if ids != vec!["41"] {
                return Reply {
                    status: 401,
                    ..Reply::json(json!({"success":false}))
                };
            }
            match call.path.as_str() {
                "/api/user/self" => ok(profile_value(41, 1)),
                "/api/user/self/groups" => groups(),
                "/api/token/" => Reply::json(json!({"success":true})),
                _ => panic!("unexpected call"),
            }
        })
        .await;
        let mut cfg = fake.config(true);
        cfg.extra_headers.push(crate::config::HeaderKV {
            key: "New-Api-User".into(),
            value: "999".into(),
        });
        let c = NewApiClient::new(&cfg).unwrap();
        c.managed_tokens(owner()).create(&spec()).await.unwrap();
        assert_eq!(writes(&fake).len(), 1);
        assert_eq!(fake.calls.lock().unwrap().len(), 3);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_uncertain_writes_are_not_automatically_retried() {
    drive(async {
        for mode in 0..4 {
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/self" => ok(profile_value(41, 1)),
                "/api/user/self/groups" => groups(),
                "/api/token/" => match mode {
                    0 => Reply {
                        status: 503,
                        body: "PRIVATE_RESPONSE".into(),
                        ..ok(Value::Null)
                    },
                    1 => Reply {
                        body: "PRIVATE_RESPONSE".into(),
                        ..ok(Value::Null)
                    },
                    2 => Reply {
                        delay: Duration::from_secs(11),
                        ..ok(Value::Null)
                    },
                    _ => Reply::json(json!({"success":false,"message":"PRIVATE_RESPONSE"})),
                },
                _ => panic!(),
            })
            .await;
            let c = client(&fake);
            let e = code(c.managed_tokens(owner()).create(&spec()).await);
            assert_eq!(
                e.kind,
                if mode == 3 {
                    Code::RemoteRejected
                } else {
                    Code::TransportUnknown
                }
            );
            assert_eq!(
                e.delivery,
                if mode == 3 {
                    DeliveryEvidence::ResponseReceived
                } else {
                    DeliveryEvidence::Uncertain
                }
            );
            assert!(!format!("{e:?}").contains("PRIVATE_RESPONSE"));
            assert_eq!(writes(&fake).len(), 1);
        }
    })
    .await;
}

#[tokio::test]
async fn t5a_expiry_elapsed_before_final_send_prevents_create() {
    let fake = Fake::start(|call| match call.path.as_str() {
        "/api/user/self" => ok(profile_value(41, 1)),
        "/api/user/self/groups" => Reply {
            delay: Duration::from_millis(2100),
            ..groups()
        },
        _ => panic!("expired before send"),
    })
    .await;
    let c = client(&fake);
    let mut s = spec();
    s.expires_at = Some(now().unwrap() + 2);
    let e = code(c.managed_tokens(owner()).create(&s).await);
    assert_eq!(e.kind, Code::RevisionConflict);
    assert_eq!(e.delivery, DeliveryEvidence::NotSent);
    assert!(writes(&fake).is_empty());
}

#[tokio::test(start_paused = true)]
async fn t5a_non_200_success_is_not_a_token_ack_or_safe_preflight() {
    drive(async {
        for status in [201, 202] {
            for stage in 0..4 {
                let fake = Fake::start(move |call| {
                    let (reply, which) = match call.path.as_str() {
                        "/api/user/self" => (ok(profile_value(41, 1)), 0),
                        "/api/user/self/groups" => (groups(), 1),
                        "/api/token/" => (Reply::json(json!({"success":true})), 2),
                        "/api/token/?p=1&page_size=100" => (ok(page(vec![], 0, 1, 100)), 3),
                        _ => panic!("unexpected endpoint"),
                    };
                    if which == stage {
                        Reply { status, ..reply }
                    } else {
                        reply
                    }
                })
                .await;
                let c = client(&fake);
                let api = c.managed_tokens(owner());
                let err = if stage == 3 {
                    code(api.list_page(1).await)
                } else {
                    code(api.create(&spec()).await)
                };
                assert_eq!(err.kind, Code::InvalidResponse);
                assert_eq!(
                    err.delivery,
                    if stage < 2 {
                        DeliveryEvidence::NotSent
                    } else {
                        DeliveryEvidence::Uncertain
                    }
                );
                assert_eq!(writes(&fake).len(), usize::from(stage == 2));
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_available_groups_are_owner_checked_and_auto_is_not_selectable() {
    drive(async {
        for mode in 0..5 {
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/self" => ok(profile_value(if mode == 4 { 42 } else { 41 }, 1)),
                "/api/user/self/groups" => match mode {
                    0 => groups(),
                    1 => ok(json!({"sgm-a":{"ratio":-1}})),
                    2 => ok(json!({"sgm-a":{"ratio":"1"}})),
                    _ => ok(json!([])),
                },
                _ => panic!("group lookup is read-only"),
            })
            .await;
            let c = client(&fake);
            let result = c.managed_tokens(owner()).available_groups().await;
            if mode == 0 {
                assert_eq!(result.unwrap(), vec!["sgm-a", "v_2"]);
            } else {
                assert_eq!(
                    code(result).kind,
                    if mode == 4 {
                        Code::AuthFailed
                    } else {
                        Code::InvalidResponse
                    }
                );
            }
            assert!(writes(&fake).is_empty());
            if mode == 4 {
                assert_eq!(fake.calls.lock().unwrap().len(), 1);
            }
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_cancel_after_send_does_not_retry_or_hold_budget() {
    drive(async {
        let posted = Arc::new(AtomicBool::new(false));
        let p = posted.clone();
        let fake = Fake::start(move |call| match call.path.as_str() {
            "/api/user/self" => ok(profile_value(41, 1)),
            "/api/user/self/groups" => groups(),
            "/api/token/" => {
                p.store(true, Ordering::SeqCst);
                Reply {
                    delay: Duration::from_secs(20),
                    ..ok(Value::Null)
                }
            }
            _ => panic!(),
        })
        .await;
        let c = Arc::new(client(&fake));
        let task_c = c.clone();
        let task =
            tokio::spawn(async move { task_c.managed_tokens(owner()).create(&spec()).await });
        while !posted.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        c.managed_tokens(owner()).profile().await.unwrap();
        assert_eq!(writes(&fake).len(), 1);
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn t5a_status_response_drift_is_uncertain_not_success() {
    drive(async {
        for field in [
            "id",
            "user_id",
            "group",
            "name",
            "expired_time",
            "unlimited_quota",
            "model_limits_enabled",
            "model_limits",
            "allow_ips",
            "cross_group_retry",
            "status",
        ] {
            let fake = Fake::start(move |call| match call.path.as_str() {
                "/api/user/self" => ok(profile_value(41, 1)),
                "/api/token/1" => ok(token(1)),
                "/api/token/?status_only=true" => {
                    let mut t = token(1);
                    t["status"] = json!(2);
                    t[field] = match field {
                        "id" | "user_id" => json!(999),
                        "expired_time" => json!(0),
                        "unlimited_quota" => json!(false),
                        "model_limits_enabled" | "cross_group_retry" => json!(true),
                        "status" => json!(1),
                        _ => json!("changed"),
                    };
                    ok(t)
                }
                _ => panic!(),
            })
            .await;
            let c = client(&fake);
            let e = code(c.managed_tokens(owner()).set_enabled(1, false).await);
            assert_eq!(e.kind, Code::InvalidResponse);
            assert_eq!(e.delivery, DeliveryEvidence::Uncertain);
            assert_eq!(writes(&fake).len(), 1);
        }
    })
    .await;
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5a_pb1_projection_matches_independent_mask_and_legacy_group(key in "[a-zA-Z0-9]{1,128}", id in 1i64..i64::MAX, own in 1i64..1000, legacy in any::<bool>(), inherited in any::<bool>(), remaining in -10000i64..10000, status in 1i64..5) {
        let o=TokenOwner{user_id:own,scope:if legacy{TokenOwnerScope::LegacyAdmin}else{TokenOwnerScope::Issuer}};
        let p=TokenOwnerProfile{user_id:own,role:if legacy{100}else{1},account_group:"owner-group".into(),quota:0};
        let mut t=token(id);t["key"]=json!(key);t["user_id"]=json!(own);t["group"]=json!(if inherited{""}else{"native-group"});t["remain_quota"]=json!(remaining);t["status"]=json!(status);
        let v=parse_token(&t,&p,o).unwrap();let n=key.len();let edge=if n<=4{0}else if n<=8{2}else{4};let stars=if edge==0{n}else if edge==2{4}else{10};
        let expected=key.chars().take(edge).chain(std::iter::repeat_n('*',stars)).chain(key.chars().skip(n-edge)).collect::<String>();
        prop_assert_eq!(&v.masked_key,&expected);prop_assert_ne!(&v.masked_key,&key);prop_assert_eq!(v.token_id,id.to_string());prop_assert_eq!(v.owner_user_id,own.to_string());prop_assert_eq!(v.remaining_quota,remaining);
        prop_assert_eq!(v.effective_group,if inherited{"owner-group"}else{"native-group"});prop_assert_eq!(v.status,[TokenStatus::Enabled,TokenStatus::Disabled,TokenStatus::Expired,TokenStatus::Exhausted][status as usize-1]);
    }
    #[test]
    fn t5a_pb2_create_payload_is_single_group_whitelist(valid in any::<bool>(),bad in 0u8..6,second_group in any::<bool>(), expiry in any::<bool>()) {
        runtime().block_on(drive(async move{
            let fake=Fake::start(|call|match call.path.as_str(){"/api/user/self"=>ok(profile_value(41,1)),"/api/user/self/groups"=>groups(),"/api/token/"=>Reply::json(json!({"success":true})),_=>panic!()}).await;
            let c=client(&fake);let mut s=spec();s.group=if second_group{"v_2"}else{"sgm-a"}.into();if expiry{s.expires_at=Some(now().unwrap()+86400);}
            if !valid {match bad{0=>s.group="auto".into(),1=>s.group="a,b".into(),2=>s.group=String::new(),3=>s.correlation_name=TAG.to_uppercase(),4=>s.expires_at=Some(JSON_SAFE+1),_=>s.expires_at=Some(0)}}
            let result=c.managed_tokens(owner()).create(&s).await;
            if valid {result.unwrap();assert_eq!(writes(&fake),vec![("POST".into(),"/api/token/".into(),json!({"name":TAG,"group":if second_group{"v_2"}else{"sgm-a"},"expired_time":s.expires_at.unwrap_or(-1),"unlimited_quota":true,"remain_quota":0,"model_limits_enabled":false,"model_limits":"","allow_ips":"","cross_group_retry":false}))]);}
            else{assert_eq!(code(result).kind,Code::InvalidInput);assert!(fake.calls.lock().unwrap().is_empty());}
        }));
    }
    #[test]
    fn t5a_pb3_owner_and_id_drift_never_produces_status_write(own in 1i64..1000,legacy in any::<bool>(), drift in 0u8..3,events in prop::collection::vec(any::<bool>(),1..5)) {
        runtime().block_on(drive(async move{
            let state=Arc::new(Mutex::new(token(7)));state.lock().unwrap()["user_id"]=json!(if drift==1{own+1000}else{own});if drift==2{state.lock().unwrap()["id"]=json!(8);}
            let s=state.clone();let fake=Fake::start(move |call|match call.path.as_str(){"/api/user/self"=>ok(profile_value(own,if legacy{100}else{1})),"/api/token/7"=>ok(s.lock().unwrap().clone()),"/api/token/?status_only=true"=>{let mut t=s.lock().unwrap();t["status"]=call.body["status"].clone();ok(t.clone())},_=>panic!()}).await;
            let c=client(&fake);let api=c.managed_tokens(TokenOwner{user_id:own,scope:if legacy{TokenOwnerScope::LegacyAdmin}else{TokenOwnerScope::Issuer}});
            let mut expected=Vec::new();for enabled in events {let result=api.set_enabled(7,enabled).await;if drift!=0{let e=code(result);assert_eq!(e.kind,Code::InvalidResponse);assert_eq!(e.delivery,DeliveryEvidence::NotSent);}else{result.unwrap();expected.push(("PUT".into(),"/api/token/?status_only=true".into(),json!({"id":7,"status":if enabled{1}else{2}})));}}
            assert_eq!(writes(&fake),expected);
        }));
    }
}

#[test]
#[ignore = "独立串行release性能门禁"]
fn performance_gate_token_maximum_page() {
    let mut rows = (1..=100).rev().map(token).collect::<Vec<_>>();
    for row in &mut rows {
        row["model_limits"] = json!("x".repeat(80_000));
    }
    let raw = page(rows, 100, 1, 100).to_string();
    assert!(raw.len() <= LIST_BODY_LIMIT);
    let started = Instant::now();
    let body: Value = serde_json::from_str(&raw).unwrap();
    let result = parse_page(&body, 1, 100, &profile(), owner()).unwrap();
    assert_eq!(result.items.len(), 100);
    assert_eq!(result.items[0].token_id, "100");
    let elapsed = started.elapsed();
    eprintln!("TOKEN_MAXIMUM_PAGE_MS={}", elapsed.as_millis());
    assert!(elapsed < Duration::from_secs(2));
}
