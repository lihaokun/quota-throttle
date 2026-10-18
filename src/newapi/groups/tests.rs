use super::*;
use crate::newapi::transport::tests::{Fake, Recorded, Reply};
use crate::newapi::{ManagementBudget, NewApiClient};
use proptest::prelude::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

fn original() -> Options {
    let ratio = BTreeMap::from([("default".into(), 1.0), ("unrelated".into(), 2.75)]);
    Options {
        ratio: ratio.clone(),
        alias: ratio,
        special: BTreeMap::from([(
            "other-account".into(),
            BTreeMap::from([("+:unrelated".into(), "原描述".into())]),
        )]),
    }
}
fn body(opts: &Options) -> Value {
    json!({"success":true,"data":[
        {"key":RATIO,"value":serde_json::to_string(&opts.ratio).unwrap()},
        {"key":ALIAS,"value":serde_json::to_string(&opts.alias).unwrap()},
        {"key":SPECIAL,"value":serde_json::to_string(&opts.special).unwrap()},
        {"key":"UntouchedOption","value":"not copied into typed options"}
    ]})
}
struct State {
    opts: Options,
    names: BTreeSet<String>,
    fail: Option<&'static str>,
    corrupt: bool,
}
impl State {
    fn reply(&mut self, call: &Recorded) -> Reply {
        match (call.method.as_str(), call.path.as_str()) {
            ("GET", "/api/group/") => Reply::json(json!({"success":true,"data":self.names})),
            ("GET", "/api/option/") => Reply::json(body(&self.opts)),
            ("PUT", "/api/option/") => {
                let key = call.body["key"].as_str().unwrap();
                if self.fail == Some(key) {
                    return Reply::json(json!({"success":false}));
                }
                let value = call.body["value"].as_str().unwrap();
                match key {
                    RATIO => {
                        self.opts.ratio = serde_json::from_str(value).unwrap();
                        self.names = self.opts.ratio.keys().cloned().collect();
                    }
                    ALIAS => {
                        self.opts.alias = serde_json::from_str(value).unwrap();
                        self.names = self.opts.alias.keys().cloned().collect();
                    }
                    SPECIAL => self.opts.special = serde_json::from_str(value).unwrap(),
                    _ => panic!("unexpected option write"),
                }
                if self.corrupt {
                    return Reply {
                        body: "{".into(),
                        ..Reply::json(Value::Null)
                    };
                }
                Reply::json(json!({"success":true}))
            }
            _ => panic!("unexpected group request"),
        }
    }
}
async fn fake(opts: Options) -> (Fake, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State {
        names: opts.ratio.keys().cloned().collect(),
        opts,
        fail: None,
        corrupt: false,
    }));
    let handler = state.clone();
    (
        Fake::start(move |r| handler.lock().unwrap().reply(r)).await,
        state,
    )
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
    panic!("bounded group scenario did not finish");
}
fn writes(fake: &Fake) -> usize {
    fake.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| c.method == "PUT")
        .count()
}

#[test]
fn t5c_group_options_reject_missing_duplicate_bad_and_oversized_maps() {
    let raw = body(&original());
    for position in 0..3 {
        let mut bad = raw.clone();
        bad["data"].as_array_mut().unwrap().remove(position);
        assert!(options(&bad).is_err());
        let mut bad = raw.clone();
        let duplicate = bad["data"][position].clone();
        bad["data"].as_array_mut().unwrap().push(duplicate);
        assert!(options(&bad).is_err());
        let mut bad = raw.clone();
        bad["data"][position]["value"] = json!("null");
        assert!(options(&bad).is_err());
    }
    for content in [
        "{\"default\":-1}",
        "{\"default\":\"1\"}",
        "[]",
        "{\"default\":1e999}",
    ] {
        let mut bad = raw.clone();
        bad["data"][0]["value"] = json!(content);
        assert!(options(&bad).is_err());
    }
    let mut large = original();
    large.ratio = (0..=LIMIT).map(|n| (format!("g{n}"), 1.0)).collect();
    assert!(options(&body(&large)).is_err());
    assert!(registered(&json!({"data":["default","default"]})).is_err());
    assert!(registered(&json!({"data":[null]})).is_err());
}
#[tokio::test(start_paused = true)]
async fn t5c_group_merge_is_idempotent_and_preserves_unrelated_options() {
    drive(async {
        let old = original();
        let (fake, state) = fake(old.clone()).await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        client
            .managed_groups(&|| true)
            .register_for("new-group", "issuer-account")
            .await
            .unwrap();
        assert_eq!(writes(&fake), 3);
        {
            let s = state.lock().unwrap();
            assert_eq!(s.opts.ratio["unrelated"], 2.75);
            assert_eq!(s.opts.alias, s.opts.ratio);
            assert_eq!(
                s.opts.special["other-account"],
                old.special["other-account"]
            );
            assert_eq!(s.opts.special["issuer-account"]["new-group"], "new-group");
        }
        client
            .managed_groups(&|| true)
            .register_for("new-group", "issuer-account")
            .await
            .unwrap();
        assert_eq!(writes(&fake), 3);
        for call in fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.method == "PUT")
        {
            assert!([RATIO, ALIAS, SPECIAL].contains(&call.body["key"].as_str().unwrap()));
        }
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_alias_ambiguity_and_denial_send_no_put() {
    drive(async {
        for known in [false, true] {
            let mut opts = original();
            opts.alias.insert("default".into(), 9.0);
            let (fake, _) = fake(opts).await;
            let client = NewApiClient::new(&fake.config(true)).unwrap();
            assert!(client
                .managed_groups(&|| true)
                .register_for(if known { "default" } else { "new-group" }, "account")
                .await
                .is_err());
            assert_eq!(writes(&fake), 0);
        }
        let mut opts = original();
        opts.special.insert(
            "account".into(),
            BTreeMap::from([("-:new-group".into(), "deny".into())]),
        );
        let (fake, _) = fake(opts).await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        assert!(client
            .managed_groups(&|| true)
            .register_for("new-group", "account")
            .await
            .is_err());
        assert_eq!(writes(&fake), 0);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_partial_or_unknown_write_never_grants_or_repairs_on_retry() {
    drive(async {
        for unknown in [false, true] {
            let (fake, state) = fake(original()).await;
            {
                let mut s = state.lock().unwrap();
                s.corrupt = unknown;
                s.fail = if unknown { None } else { Some(ALIAS) };
            }
            let client = NewApiClient::new(&fake.config(true)).unwrap();
            assert!(client
                .managed_groups(&|| true)
                .register_for("new-group", "account")
                .await
                .is_err());
            let written = writes(&fake);
            assert_eq!(written, if unknown { 1 } else { 2 });
            assert!(state.lock().unwrap().names.contains("new-group"));
            assert!(!state.lock().unwrap().opts.special.contains_key("account"));
            assert!(client
                .managed_groups(&|| true)
                .register_for("new-group", "account")
                .await
                .is_err());
            assert_eq!(writes(&fake), written);
        }
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_final_guard_invalidates_before_any_write() {
    drive(async {
        let active = Arc::new(AtomicBool::new(true));
        let signal = active.clone();
        let fake = Fake::start(move |r| match r.path.as_str() {
            "/api/group/" => Reply::json(json!({"success":true,"data":["default","unrelated"]})),
            "/api/option/" if r.method == "GET" => {
                signal.store(false, Ordering::SeqCst);
                Reply::json(body(&original()))
            }
            _ => panic!("stale guard sent a write"),
        })
        .await;
        let client = NewApiClient::new(&fake.config(true)).unwrap();
        let guard = || active.load(Ordering::SeqCst);
        assert!(client
            .managed_groups(&guard)
            .register_for("new-group", "account")
            .await
            .is_err());
        assert_eq!(writes(&fake), 0);
    })
    .await;
}
#[tokio::test(start_paused = true)]
async fn t5c_group_invalid_input_is_network_free_and_scope_requires_same_budget() {
    drive(async {
        let fake = Fake::start(|_| panic!("invalid input sends nothing")).await;
        let budget = ManagementBudget::new();
        let config = fake.config(true);
        let a = NewApiClient::with_budget(&config, budget.clone()).unwrap();
        let b = NewApiClient::with_budget(&config, budget.clone()).unwrap();
        let c = NewApiClient::new(&config).unwrap();
        assert!(a.shares_management_scope(&b));
        assert!(!a.shares_management_scope(&c));
        let mut other = config;
        other.base_url = "http://127.0.0.1:9".into();
        assert!(!a.shares_management_scope(&NewApiClient::with_budget(&other, budget).unwrap()));
        for group in ["", "auto", "one,two"] {
            assert!(a
                .managed_groups(&|| true)
                .register_for(group, "account")
                .await
                .is_err());
        }
        assert!(a
            .managed_groups(&|| true)
            .register_for("new-group", "")
            .await
            .is_err());
        assert!(fake.calls.lock().unwrap().is_empty());
    })
    .await;
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn t5c_pb1_group_merge_matches_independent_add_only_reference(entries in prop::collection::btree_map(0u16..1000,0u16..2000,0..40)) {
        let old=Ratios::from_iter(entries.iter().map(|(k,v)|(format!("keep-{k}"),f64::from(*v)/8.0)));
        let opts=Options{ratio:old.clone(),alias:old.clone(),special:BTreeMap::new()};
        let mut expected=old.clone(); expected.insert("new-group".into(),1.0);
        prop_assert_eq!(add_ratio(&opts,&old.keys().cloned().collect(),"new-group").unwrap(),Some(expected));
        let prior=Special::from_iter(entries.keys().map(|k|(format!("account-{k}"),BTreeMap::from([("+:keep".into(),format!("desc-{k}"))]))));
        let mut expected=prior.clone(); expected.insert("issuer".into(),BTreeMap::from([("new-group".into(),"new-group".into())]));
        let next=add_permission(&prior,"issuer","new-group").unwrap().unwrap();
        prop_assert_eq!(&next,&expected);
        prop_assert!(add_permission(&next,"issuer","new-group").unwrap().is_none());
    }
    #[test]
    fn t5c_pb2_group_conflicts_never_choose_alias_or_remove_deny(ratio in 0u16..10000, positive in any::<bool>(), prefix in any::<bool>()) {
        let mut opts=original(); opts.ratio.insert("default".into(),f64::from(ratio)); opts.alias.insert("default".into(),f64::from(ratio)+1.0);
        prop_assert!(add_ratio(&opts,&opts.ratio.keys().cloned().collect(),"new-group").is_err());
        let mut scope=BTreeMap::from([("-:new-group".into(),"explicit deny".into())]);
        if positive {scope.insert(if prefix {"+:new-group"} else {"new-group"}.into(),"keep description".into());}
        let old=BTreeMap::from([("account".into(),scope)]); let snapshot=old.clone();
        prop_assert!(add_permission(&old,"account","new-group").is_err());
        prop_assert_eq!(old,snapshot);
    }
}
#[test]
#[ignore = "独立串行release性能门禁"]
fn performance_gate_group_options_10k() {
    let ratio: Ratios = (0..9999).map(|n| (format!("keep-{n}"), 1.25)).collect();
    let special = Special::from([(
        "issuer".into(),
        (0..9999)
            .map(|n| (format!("keep-{n}"), "保持".into()))
            .collect(),
    )]);
    let raw = body(&Options {
        ratio: ratio.clone(),
        alias: ratio,
        special,
    });
    let started = Instant::now();
    let old = options(&raw).unwrap();
    assert_eq!(
        add_ratio(&old, &old.ratio.keys().cloned().collect(), "new-group")
            .unwrap()
            .unwrap()
            .len(),
        10000
    );
    assert_eq!(
        add_permission(&old.special, "issuer", "new-group")
            .unwrap()
            .unwrap()["issuer"]
            .len(),
        10000
    );
    println!("GROUP_OPTIONS_10K_MS={}", started.elapsed().as_millis());
    assert!(started.elapsed() < Duration::from_secs(5));
}
