use super::*;
use proptest::prelude::*;
use serde_json::json;

fn query() -> LogPageQuery {
    LogPageQuery::new(ScanWindow { from: 100, to: 200 }, 1).unwrap()
}
fn body(rows: Vec<Value>, total: i64) -> Value {
    json!({"success":true,"data":{"page":1,"page_size":100,"total":total,"items":rows}})
}
fn rows(n: usize) -> Vec<Value> {
    (1..=n)
        .rev()
        .map(|id| json!({"id":id,"created_at":125,"type":3}))
        .collect()
}

#[test]
fn log_pages_validate_every_raw_row_and_do_not_trust_total() {
    for (count, total, end) in [
        (100, 0, PageEnd::More),
        (0, 900, PageEnd::ReachedEnd),
        (1, 0, PageEnd::ReachedEnd),
    ] {
        let p = decode_log_page(body(rows(count), total), &query()).unwrap();
        assert_eq!(p.end, end);
        assert_eq!(p.raw_count, count);
    }
    let valid = body(rows(2), 2);
    for (name, value) in [
        ("id", json!(0)),
        ("created_at", json!(99)),
        ("created_at", json!(200)),
        ("type", json!(-1)),
        ("type", json!("3")),
    ] {
        let mut b = valid.clone();
        b["data"]["items"][1][name] = value;
        assert_eq!(
            decode_log_page(b, &query()).err(),
            Some(CollectionError::InvalidPage)
        );
    }
    for field in ["id", "created_at", "type"] {
        let mut b = valid.clone();
        b["data"]["items"][1].as_object_mut().unwrap().remove(field);
        assert!(decode_log_page(b, &query()).is_err());
    }
    let mut duplicate = rows(2);
    duplicate[1]["id"] = json!(2);
    duplicate[1]["created_at"] = json!(124);
    assert!(decode_log_page(body(duplicate, 2), &query()).is_err());
    let mut unordered = rows(2);
    unordered.reverse();
    assert!(decode_log_page(body(unordered, 2), &query()).is_err());
    for name in ["page", "page_size", "total", "items"] {
        let mut b = valid.clone();
        b["data"].as_object_mut().unwrap().remove(name);
        assert!(decode_log_page(b, &query()).is_err());
    }
    for (name, value) in [
        ("page", json!(2)),
        ("page_size", json!(99)),
        ("total", json!(-1)),
        ("total", json!(JSON_SAFE + 1)),
        ("items", json!(null)),
    ] {
        let mut b = valid.clone();
        b["data"][name] = value;
        assert!(decode_log_page(b, &query()).is_err());
    }
    let mut b = valid.clone();
    b["success"] = json!(false);
    assert!(decode_log_page(b, &query()).is_err());
    assert!(decode_log_page(body(rows(101), 101), &query()).is_err());
    let b = body(vec![json!({"id":i64::MAX,"created_at":100,"type":7})], 1);
    assert!(decode_log_page(b, &query()).is_ok());
}
#[test]
fn log_page_query_bounds_and_movement_are_explicit() {
    assert_eq!(
        query().path(),
        "/api/log/?type=0&p=1&page_size=100&start_timestamp=100&end_timestamp=199"
    );
    for (from, to, page) in [
        (0, 1, 1),
        (-1, 2, 1),
        (2, 2, 1),
        (0, JSON_SAFE + 1, 1),
        (0, 2, 0),
        (0, 2, i64::MAX),
    ] {
        assert!(LogPageQuery::new(ScanWindow { from, to }, page).is_err());
    }
    assert!(LogPageQuery::new(ScanWindow { from: 0, to: 2 }, 1)
        .unwrap()
        .into_request()
        .is_ok());
    let p = decode_log_page(body(rows(2), 2), &query()).unwrap();
    assert!(!observe_page_change(None, &p));
    assert!(observe_page_change(Some(p.observation()), &p));
    assert!(!observe_page_change(
        Some(PageObservation {
            total: 2,
            last_key: Some((126, 1))
        }),
        &p
    ));
    assert!(observe_page_change(
        Some(PageObservation {
            total: 3,
            last_key: None
        }),
        &p
    ));
    let empty = decode_log_page(body(vec![], 2), &query()).unwrap();
    assert!(!observe_page_change(Some(p.observation()), &empty));
}
#[test]
fn log_page_private_payload_has_no_debug_or_serde() {
    use crate::newapi::transport::assurance_tests::assert_not_impl;
    assert_not_impl!(CheckedLogPage, std::fmt::Debug);
    assert_not_impl!(CheckedLogPage, serde::Serialize);
    assert_not_impl!(CheckedLogPage, serde::Deserialize<'static>);
    assert_not_impl!(LogPageQuery, std::fmt::Debug);
    assert_not_impl!(LogPageQuery, serde::Deserialize<'static>);
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn pb1_log_page_raw_count_and_endpoint_contract(n in 0usize..103, from in 0i64..1_000_000, width in 2i64..500, page in 1i64..1_000_000, total in 0i64..100000, large in any::<bool>(), defect in 0u8..10) {
        let q=LogPageQuery::new(ScanWindow{from,to:from+width},page).unwrap();
        let expected_suffix=format!("start_timestamp={from}&end_timestamp={}",from+width-1);
        prop_assert!(q.path().ends_with(&expected_suffix));
        let raw:Vec<_>=(0..n).rev().map(|i|json!({"id":if large{JSON_SAFE+i as i64+1}else{i as i64+1},"created_at":from,"type":([2,5,7][i%3])})).collect();
        let mut b=body(raw,total);b["data"]["page"]=json!(page);
        let mut bad=b.clone();
        match defect {
            0=>bad["success"]=json!("true"),
            1=>bad["data"]["page"]=json!(page+1),
            2=>bad["data"]["page_size"]=json!(99),
            3=>bad["data"]["total"]=json!(JSON_SAFE+1),
            4=>{bad["data"].as_object_mut().unwrap().remove("items");},
            _ if n==0=>bad["data"]["items"]=json!(null),
            5=>bad["data"]["items"][n-1]["id"]=json!(0),
            6=>bad["data"]["items"][n-1]["created_at"]=json!(from+width),
            7=>bad["data"]["items"][n-1]["type"]=json!(-1),
            8=>{bad["data"]["items"][n-1].as_object_mut().unwrap().remove("created_at");},
            _ if n>1=>{bad["data"]["items"][n-1]=bad["data"]["items"][0].clone();},
            _=>bad["data"]["items"][0]["id"]=json!(1.5),
        }
        prop_assert_eq!(decode_log_page(bad,&q).err(),Some(CollectionError::InvalidPage));
        let got=decode_log_page(b,&q);
        prop_assert_eq!(got.is_ok(),n<=100);
        if let Ok(got)=got {prop_assert_eq!(got.end,if n==100{PageEnd::More}else{PageEnd::ReachedEnd});prop_assert_eq!(got.raw_count,n);}
    }
}

#[tokio::test]
async fn log_read_reuses_authentication_and_only_reads_global_pages() {
    use crate::newapi::transport::tests::{Fake, Reply};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let reads = AtomicUsize::new(0);
    let fake = Fake::start(move |r| {
        if r.path == "/api/user/login" {
            return Reply::json(json!({"success":true,"data":{"id":42}}));
        }
        assert!(r.path.starts_with("/api/log/?type=0&p=1&page_size=100&"));
        if reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply {
                status: 401,
                ..Reply::json(json!({}))
            }
        } else {
            Reply::json(body(rows(100), 0))
        }
    })
    .await;
    let client = NewApiClient::new(&fake.config(false)).unwrap();
    let lease = LocalLogLease::test_fixture();
    let page = client
        .read_log_page(query(), &lease, RequestClass::Observe, &|| true)
        .await
        .unwrap();
    assert_eq!(page.end, PageEnd::More);
    let calls = fake.calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|r| r.path == "/api/user/login").count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/api/log/"))
            .count(),
        2
    );
}
#[tokio::test]
async fn log_read_rejects_control_class_and_preserves_429_even_with_oversize_body() {
    use crate::newapi::transport::tests::{Fake, Reply};
    let fake = Fake::start(|_| Reply {
        status: 429,
        body: "secret".repeat(LIST_BODY_LIMIT / 6 + 1),
        headers: "Retry-After: 20\r\n".into(),
        delay: std::time::Duration::ZERO,
    })
    .await;
    let client = NewApiClient::new(&fake.config(true)).unwrap();
    let lease = LocalLogLease::test_fixture();
    assert_eq!(
        client
            .read_log_page(query(), &lease, RequestClass::Control, &|| true)
            .await
            .err(),
        Some(CollectionError::InvalidPage)
    );
    assert!(fake.calls.lock().unwrap().is_empty());
    assert!(client
        .read_log_page(query(), &lease, RequestClass::Backfill, &|| false)
        .await
        .is_err());
    assert!(fake.calls.lock().unwrap().is_empty());
    let error = client
        .read_log_page(query(), &lease, RequestClass::Backfill, &|| true)
        .await
        .err()
        .unwrap();
    match &error {
        CollectionError::Management(e) => {
            assert_eq!(e.status, Some(429));
            assert_eq!(e.retry_after, Some(std::time::Duration::from_secs(20)));
        }
        _ => panic!("expected safe management error"),
    }
    assert!(!format!("{error:?}").contains("secret"));
}

#[tokio::test]
async fn log_read_401_replay_rechecks_cancellation() {
    use crate::newapi::transport::{
        tests::{Fake, Reply},
        ManagementErrorKind,
    };
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    let allowed = Arc::new(AtomicBool::new(true));
    let check = allowed.clone();
    let logins = AtomicUsize::new(0);
    let fake = Fake::start(move |r| {
        if r.path == "/api/user/login" {
            if logins.fetch_add(1, Ordering::SeqCst) == 1 {
                check.store(false, Ordering::SeqCst);
            }
            Reply::json(json!({"success":true,"data":{"id":42}}))
        } else {
            Reply {
                status: 401,
                ..Reply::json(json!({}))
            }
        }
    })
    .await;
    let client = NewApiClient::new(&fake.config(false)).unwrap();
    let lease = LocalLogLease::test_fixture();
    let guard = || allowed.load(Ordering::SeqCst);
    let error = client
        .read_log_page(query(), &lease, RequestClass::Observe, &guard)
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error,CollectionError::Management(ref e) if e.kind==ManagementErrorKind::RevisionConflict),
        "{error:?}"
    );
    let calls = fake.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|r| r.path.starts_with("/api/log/"))
            .count(),
        1
    );
    assert_eq!(
        calls.iter().filter(|r| r.path == "/api/user/login").count(),
        2
    );
}

#[tokio::test]
async fn log_read_expired_lease_is_checked_after_budget_wait() {
    use crate::newapi::transport::{
        tests::{queued_requests, Fake, Reply},
        ManagementBudget, ManagementErrorKind,
    };
    use std::time::Duration;
    let fake = Fake::start(|r| {
        if r.path == "/api/cooldown" {
            Reply {
                status: 429,
                headers: "Retry-After: 60\r\n".into(),
                ..Reply::json(json!({"success":false}))
            }
        } else {
            Reply::json(body(vec![], 0))
        }
    })
    .await;
    let budget = ManagementBudget::new();
    let client = NewApiClient::with_budget(&fake.config(true), budget.clone()).unwrap();
    let request = ManagementRequest::new(
        Method::GET,
        "/api/cooldown",
        None,
        RequestSemantics::Read,
        LIST_BODY_LIMIT,
    )
    .unwrap();
    assert!(client
        .transport
        .send(request, RequestClass::Observe)
        .await
        .is_err());
    let lease = LocalLogLease::test_fixture();
    let advance = async {
        tokio::time::timeout(Duration::from_secs(10), async {
            while queued_requests(&budget) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
    };
    let (result, ()) = tokio::join!(
        client.read_log_page(query(), &lease, RequestClass::Backfill, &|| true),
        advance
    );
    assert!(
        matches!(result.err().unwrap(),CollectionError::Management(e) if e.kind==ManagementErrorKind::RevisionConflict)
    );
    assert_eq!(fake.calls.lock().unwrap().len(), 1);
    assert_eq!(queued_requests(&budget), 0);
}
