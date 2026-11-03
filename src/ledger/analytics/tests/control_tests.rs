use super::*;
use std::sync::{mpsc, Arc, Barrier};

#[test]
fn query_control_cancel_before_registration_and_registration_race_are_safe() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let c = control();
    c.cancel();
    assert_eq!(
        c.register_interrupt(db.get_interrupt_handle()),
        Err(AnalyticsError::Cancelled)
    );
    for _ in 0..128 {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        let c = Arc::new(control());
        let barrier = Arc::new(Barrier::new(2));
        let other = c.clone();
        let gate = barrier.clone();
        let cancel = std::thread::spawn(move || {
            gate.wait();
            other.cancel();
        });
        barrier.wait();
        let result = c.register_interrupt(db.get_interrupt_handle());
        cancel.join().unwrap();
        assert!(result.is_ok() || result == Err(AnalyticsError::Cancelled));
        assert_eq!(c.check(), Err(AnalyticsError::Cancelled));
        drop(db);
        c.cancel(); // 连接关闭后句柄也不能触碰已释放 SQLite 指针。
    }
}

#[test]
fn query_control_interrupts_actual_sql_after_registration() {
    use rusqlite::ffi;
    struct Gate {
        ready: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    unsafe extern "C" fn pause(
        ctx: *mut ffi::sqlite3_context,
        _argc: i32,
        _argv: *mut *mut ffi::sqlite3_value,
    ) {
        let gate = unsafe { &*(ffi::sqlite3_user_data(ctx) as *const Gate) };
        let _ = gate.ready.send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(10));
        unsafe { ffi::sqlite3_result_int(ctx, 0) };
    }
    unsafe extern "C" fn destroy(p: *mut std::ffi::c_void) {
        drop(unsafe { Box::from_raw(p as *mut Gate) });
    }
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let c = control();
    c.register_interrupt(db.get_interrupt_handle()).unwrap();
    let (ready_tx, ready) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let gate = Box::into_raw(Box::new(Gate {
        ready: ready_tx,
        release: wait,
    }));
    let rc = unsafe {
        ffi::sqlite3_create_function_v2(
            db.handle(),
            c"query_gate".as_ptr(),
            0,
            ffi::SQLITE_UTF8,
            gate.cast(),
            Some(pause),
            None,
            None,
            Some(destroy),
        )
    };
    assert_eq!(rc, ffi::SQLITE_OK);
    let task = std::thread::spawn(move || {
        db.query_row::<i64,_,_>("WITH RECURSIVE n(x) AS (SELECT query_gate() UNION ALL SELECT x+1 FROM n WHERE x<1000000) SELECT sum(x) FROM n",[],|r|r.get(0))
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    c.cancel();
    release.send(()).unwrap();
    let e = task.join().unwrap().unwrap_err();
    assert_eq!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::OperationInterrupted)
    );
}
