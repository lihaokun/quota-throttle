use super::*;
use proptest::prelude::*;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncWriteExt, ReadBuf};

pub(crate) struct Chunks {
    bytes: Vec<u8>,
    offset: usize,
    widths: Vec<usize>,
    step: usize,
}
impl Chunks {
    pub(crate) fn new(bytes: Vec<u8>, widths: Vec<usize>) -> Self {
        Self {
            bytes,
            offset: 0,
            widths,
            step: 0,
        }
    }
}
impl AsyncRead for Chunks {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = self.widths[self.step % self.widths.len()]
            .min(buf.remaining())
            .min(self.bytes.len() - self.offset);
        buf.put_slice(&self.bytes[self.offset..self.offset + n]);
        self.offset += n;
        self.step += 1;
        Poll::Ready(Ok(()))
    }
}
use std::io;
fn base() -> String {
    "GET /api/admin/v1/state HTTP/1.1\r\nHost: localhost:3001\r\nX-QT-Panel: 1\r\n\r\n".into()
}
fn with_header(line: &str) -> String {
    base().replace("\r\n\r\n", &format!("\r\n{line}\r\n\r\n"))
}

#[test]
fn h1_origin_and_authority_matrix_never_trusts_request_aliases() {
    for (input, canonical) in [
        ("http://127.0.0.1:3001", "http://127.0.0.1:3001"),
        ("HTTP://LOCALHOST:80/", "http://localhost"),
        ("http://[::1]:3001/", "http://[::1]:3001"),
    ] {
        let origin = PanelOrigin::parse(input).unwrap();
        assert_eq!(origin.origin, canonical);
    }
    for input in [
        "https://localhost:3001",
        "http://example.com",
        "http://localhost.evil",
        "http://localhost./",
        "http://user@localhost",
        "http://@localhost",
        "http://localhost/x",
        "http://localhost/a/..",
        "http://localhost/?q=1",
        "http://localhost/#f",
        "http://localhost:0",
        "http://localhost:65536",
        "http://localhost\\evil",
        " http://localhost",
    ] {
        assert!(PanelOrigin::parse(input).is_err(), "{input}");
    }
    let origin = PanelOrigin::parse("http://localhost:3001").unwrap();
    for host in [
        "127.0.0.1:3001",
        "LOCALHOST:3001",
        "localhost:3002",
        "localhost:03001",
        "localhost.evil:3001",
    ] {
        let head = parse_head(base().replace("localhost:3001", host).as_bytes()).unwrap();
        assert_eq!(authorize(&head, &origin), Err(Code::Forbidden));
    }
}

#[test]
fn h1_strict_head_rejects_ambiguity_invalid_lengths_and_control_bytes() {
    let cases = [
        (with_header("hOsT: localhost:3001"), Code::BadRequest),
        (with_header("X-QT-Panel: 1"), Code::BadRequest),
        (
            with_header("Content-Length: 0\r\ncOnTeNt-LeNgTh: 0"),
            Code::BadRequest,
        ),
        (
            with_header("Origin: http://localhost:3001\r\norigin: null"),
            Code::BadRequest,
        ),
        (with_header("Content-Length: 0,0"), Code::BadRequest),
        (with_header("Content-Length: +0"), Code::BadRequest),
        (with_header("Content-Length: -1"), Code::BadRequest),
        (with_header("Content-Length: xyz"), Code::BadRequest),
        (with_header("Content-Length:"), Code::BadRequest),
        (
            with_header("Content-Length: 18446744073709551616"),
            Code::BadRequest,
        ),
        (with_header("Content-Length: 1048577"), Code::BodyTooLarge),
        (with_header("Content-Length: 1"), Code::BadRequest),
        (
            with_header("Transfer-Encoding: chunked\r\nContent-Length: 0"),
            Code::BadRequest,
        ),
        (with_header("Transfer-Encoding: identity"), Code::BadRequest),
        (with_header("Upgrade: websocket"), Code::BadRequest),
        (
            with_header("Connection: Keep-Alive, UpGrade"),
            Code::BadRequest,
        ),
        (
            with_header("Content-Encoding: identity"),
            Code::UnsupportedMediaType,
        ),
        (with_header("Expect: 100-continue"), Code::ExpectationFailed),
        (with_header("X-Value : x"), Code::BadRequest),
        (with_header(" X-Value: x"), Code::BadRequest),
        (
            with_header("X-Value: hi\r\n continuation"),
            Code::BadRequest,
        ),
        (with_header("X-Value: hi\nthere"), Code::BadRequest),
        (
            base().replace("HTTP/1.1", "HTTP/1.0"),
            Code::UnsupportedVersion,
        ),
        (
            base().replace("HTTP/1.1", "HTTP/2.0"),
            Code::UnsupportedVersion,
        ),
        (base().replace("HTTP/1.1", "HTTP/1.11"), Code::BadRequest),
        (base().replace("GET ", "GET\t"), Code::BadRequest),
        (
            base().replace("Host: localhost:3001\r\n", ""),
            Code::BadRequest,
        ),
        (
            base().replace("Host: localhost:3001", "Host: localhost:abc"),
            Code::BadRequest,
        ),
        (base().replace("GET ", "POST "), Code::LengthRequired),
    ];
    for (bytes, code) in cases {
        assert_eq!(parse_head(bytes.as_bytes()).err(), Some(code), "{bytes:?}");
    }
    for path in [
        "http://localhost:3001/x",
        "//api/admin/v1/state",
        "/a/../state",
        "/./state",
        "/api/%61dmin",
        "/a\\b",
        "/a#frag",
        "/x?hi\0there",
        "/x?hi\tthere",
        "/x?hi\x7fthere",
        "/x?值",
    ] {
        assert_eq!(
            parse_head(base().replace("/api/admin/v1/state", path).as_bytes()).err(),
            Some(Code::BadRequest)
        );
    }
    assert!(parse_head(with_header("Content-Length: 000000").as_bytes()).is_ok());
    let empty = parse_head(base().replace("/state ", "/state? ").as_bytes()).unwrap();
    assert_eq!(empty.query, Some(String::new()));
    assert_eq!(parse_head(base().as_bytes()).unwrap().query, None);
}

#[test]
fn h1_head_target_and_field_limits_include_exact_boundaries() {
    assert_eq!(MAX_HEAD, 16 * 1024);
    assert_eq!(MAX_TARGET, 8 * 1024);
    assert_eq!(MAX_FIELDS, 64);
    assert_eq!(MAX_BODY, 1024 * 1024);
    assert_eq!(HEAD_TIMEOUT, Duration::from_secs(5));
    assert_eq!(BODY_TIMEOUT, Duration::from_secs(10));
    let extra = "X-Pad: \r\n";
    let pre = base().replace("\r\n\r\n", &format!("\r\n{extra}\r\n"));
    let exact = pre.replace(
        "X-Pad: \r\n",
        &format!("X-Pad: {}\r\n", "a".repeat(MAX_HEAD - pre.len())),
    );
    assert_eq!(exact.len(), MAX_HEAD);
    assert!(parse_head(exact.as_bytes()).is_ok());
    assert_eq!(
        parse_head(exact.replacen("X-Pad: a", "X-Pad: aa", 1).as_bytes()).err(),
        Some(Code::HeaderTooLarge)
    );
    let path = format!("/{}", "a".repeat(MAX_TARGET - 1));
    assert!(parse_head(base().replace("/api/admin/v1/state", &path).as_bytes()).is_ok());
    assert_eq!(
        parse_head(
            base()
                .replace("/api/admin/v1/state", &(path + "a"))
                .as_bytes()
        )
        .err(),
        Some(Code::TargetTooLong)
    );
    for count in [62, 63] {
        let lines = (0..count)
            .map(|i| format!("X-{i}: x\r\n"))
            .collect::<String>();
        let text = base().replace("\r\n\r\n", &format!("\r\n{lines}\r\n"));
        assert_eq!(
            parse_head(text.as_bytes()).err(),
            if count == 62 {
                None
            } else {
                Some(Code::HeaderTooLarge)
            }
        );
    }
}

#[tokio::test]
async fn h1_oversized_delimiter_in_same_read_cannot_bypass_limit() {
    let text = with_header(&format!("X-Pad: {}", "x".repeat(MAX_HEAD)));
    // 3000不整除head上限：同一次read跨过上限并读到delimiter。
    for width in [1, 3000, 4096, 100000] {
        let mut input = Chunks::new(text.as_bytes().to_vec(), vec![width]);
        assert_eq!(
            read_head(&mut input, Instant::now()).await.err(),
            Some(RequestError::Reject(Code::HeaderTooLarge, false))
        );
    }
}

#[test]
fn h1_origin_csrf_fetch_and_json_guards_are_independent() {
    let origin = PanelOrigin::parse("http://localhost:3001").unwrap();
    assert!(authorize(&parse_head(base().as_bytes()).unwrap(), &origin).is_ok());
    for line in [
        "Origin: null",
        "Origin: http://localhost:3001.evil",
        "Origin: http://localhost:3001/",
        "Origin: http://localhost:3001 http://evil",
        "Sec-Fetch-Site: cross-site",
        "Sec-Fetch-Site: same-site",
        "Sec-Fetch-Site: unknown",
    ] {
        assert_eq!(
            authorize(&parse_head(with_header(line).as_bytes()).unwrap(), &origin),
            Err(Code::Forbidden)
        );
    }
    for method in ["POST", "PUT", "DELETE"] {
        let write = with_header("Content-Length: 0").replace("GET ", &format!("{method} "));
        assert_eq!(
            authorize(&parse_head(write.as_bytes()).unwrap(), &origin),
            Err(Code::Forbidden)
        );
        let write = write.replace("\r\n\r\n", "\r\nOrigin: http://localhost:3001\r\n\r\n");
        for value in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "application/json; charset=latin1",
            "application/json; charset=utf-8; x=y",
            "",
        ] {
            let text = write.replace("\r\n\r\n", &format!("\r\nContent-Type: {value}\r\n\r\n"));
            assert_eq!(
                authorize(&parse_head(text.as_bytes()).unwrap(), &origin),
                Err(Code::UnsupportedMediaType)
            );
        }
        for value in ["application/json", "APPLICATION/JSON; CHARSET=UTF-8"] {
            let text = write.replace("\r\n\r\n", &format!("\r\nContent-Type: {value}\r\n\r\n"));
            assert!(authorize(&parse_head(text.as_bytes()).unwrap(), &origin).is_ok());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn h1_slow_head_body_eof_and_utf8_fail_without_partial_success() {
    let (mut peer, mut input) = tokio::io::duplex(64);
    peer.write_all(b"HEAD /api/admin/v1/state HTTP/1.1\r\n")
        .await
        .unwrap();
    let start = Instant::now();
    assert_eq!(
        read_head(&mut input, start).await.err(),
        Some(RequestError::Reject(Code::RequestTimeout, true))
    );
    assert_eq!(Instant::now() - start, Duration::from_secs(5));
    let (mut peer, mut input) = tokio::io::duplex(64);
    peer.write_all(b"a").await.unwrap();
    let start = Instant::now();
    assert_eq!(
        read_body(&mut input, vec![], 2, start + BODY_TIMEOUT)
            .await
            .err(),
        Some(RequestError::Reject(Code::RequestTimeout, false))
    );
    assert_eq!(Instant::now() - start, BODY_TIMEOUT);
    drop(peer);
    assert_eq!(
        read_body(&mut input, vec![], 1, Instant::now() + BODY_TIMEOUT)
            .await
            .err(),
        Some(RequestError::Reject(Code::BadRequest, false))
    );
    let mut input = Chunks::new(vec![0xff], vec![1]);
    assert_eq!(
        read_body(&mut input, vec![], 1, Instant::now() + BODY_TIMEOUT)
            .await
            .err(),
        Some(RequestError::Reject(Code::BadRequest, false))
    );
    let mut input = Chunks::new(vec![], vec![1]);
    assert_eq!(
        read_head(&mut input, Instant::now()).await.err(),
        Some(RequestError::Reject(Code::BadRequest, false))
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]
    #[test]
    fn h1_pb1_single_message_framing_is_partition_invariant(text in ".{0,160}", widths in prop::collection::vec(1usize..200,1..12)) {
        let body=serde_json::to_vec(&serde_json::json!({"name":text})).unwrap();
        let head=format!("POST /api/admin/v1/test HTTP/1.1\r\nHost: localhost:3001\r\nContent-Length: {}\r\n\r\n",body.len());
        let mut bytes=head.as_bytes().to_vec();bytes.extend(&body);bytes.extend(b"NEXT REQUEST MUST NOT BE CONSUMED");
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let mut input=Chunks::new(bytes,widths);let (head,buffer,_)=read_head(&mut input,Instant::now()).await.unwrap();
            prop_assert_eq!(&head.method,"POST");prop_assert_eq!(head.content_length,Some(body.len()));prop_assert_eq!(head.query,None);
            let actual=read_body(&mut input,buffer,head.content_length.unwrap(),Instant::now()+BODY_TIMEOUT).await.unwrap();prop_assert_eq!(actual,body);Ok(())
        })?;
    }
    #[test]
    fn h1_pb2_untrusted_header_combinations_never_authorize(host_bad in any::<bool>(), origin_kind in 0u8..4, marker in 0u8..3, metadata in 0u8..4, writing_request in any::<bool>(), duplicate in any::<bool>()) {
        let host=if host_bad {"evil.example:3001"}else{"localhost:3001"};
        let mut lines=format!("{} /api/admin/v1/state HTTP/1.1\r\nHost: {host}\r\n",if writing_request {"POST"}else{"GET"});
        if marker>0 {lines.push_str(if marker==1 {"X-QT-Panel: 1\r\n"}else{"X-QT-Panel: yes\r\n"});}
        if origin_kind>0 {lines.push_str(match origin_kind {1=>"Origin: http://localhost:3001\r\n",2=>"Origin: null\r\n",_=>"Origin: http://localhost:3002\r\n"});}
        if metadata>0 {lines.push_str(match metadata {1=>"Sec-Fetch-Site: same-origin\r\n",2=>"Sec-Fetch-Site: none\r\n",_=>"Sec-Fetch-Site: same-site\r\n"});}
        if writing_request {lines.push_str("Content-Length: 0\r\nContent-Type: application/json\r\n");}
        if duplicate {lines.push_str("hOsT: localhost:3001\r\n");} lines.push_str("\r\n");
        let allowed=!host_bad && marker==1 && (origin_kind==1 || (!writing_request && origin_kind==0)) && metadata!=3 && !duplicate;
        let actual=parse_head(lines.as_bytes()).ok().is_some_and(|h|authorize(&h,&PanelOrigin::parse("http://localhost:3001").unwrap()).is_ok());
        prop_assert_eq!(actual,allowed);
    }
}

#[tokio::test(start_paused = true)]
async fn h1_ready_head_cannot_succeed_at_absolute_deadline() {
    let mut input = Chunks::new(base().into_bytes(), vec![4096]);
    let accepted = Instant::now() - HEAD_TIMEOUT;
    assert_eq!(
        read_head(&mut input, accepted).await.err(),
        Some(RequestError::Reject(Code::RequestTimeout, false))
    );
    assert_eq!(input.offset, 0);
}

#[tokio::test(start_paused = true)]
async fn h1_ready_or_buffered_body_cannot_succeed_at_absolute_deadline() {
    for buffered in [false, true] {
        let mut input = Chunks::new(if buffered { vec![] } else { b"{}".to_vec() }, vec![4096]);
        let prefix = if buffered { b"{}".to_vec() } else { vec![] };
        assert_eq!(
            read_body(&mut input, prefix, 2, Instant::now()).await.err(),
            Some(RequestError::Reject(Code::RequestTimeout, false))
        );
        assert_eq!(input.offset, 0);
    }
}

struct DelayedRead {
    ready: Pin<Box<tokio::time::Sleep>>,
    bytes: Vec<u8>,
}
impl AsyncRead for DelayedRead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        use std::future::Future;
        std::task::ready!(self.ready.as_mut().poll(cx));
        let n = self.bytes.len().min(buf.remaining());
        buf.put_slice(&self.bytes[..n]);
        self.bytes.drain(..n);
        Poll::Ready(Ok(()))
    }
}
#[tokio::test(start_paused = true)]
async fn h1_read_returning_ready_at_deadline_does_not_become_success() {
    let start = Instant::now();
    let mut head = DelayedRead {
        ready: Box::pin(tokio::time::sleep_until(start + HEAD_TIMEOUT)),
        bytes: base().into_bytes(),
    };
    assert_eq!(
        read_head(&mut head, start).await.err(),
        Some(RequestError::Reject(Code::RequestTimeout, false))
    );
    let end = Instant::now() + BODY_TIMEOUT;
    let mut body = DelayedRead {
        ready: Box::pin(tokio::time::sleep_until(end)),
        bytes: b"{}".to_vec(),
    };
    assert_eq!(
        read_body(&mut body, vec![], 2, end).await.err(),
        Some(RequestError::Reject(Code::RequestTimeout, false))
    );
}
#[tokio::test(start_paused = true)]
async fn h1_body_deadline_is_anchored_at_head_completion_including_buffered_body() {
    let start = Instant::now();
    let bytes = with_header("Content-Length: 2").replace("GET ", "POST ") + "{}";
    let mut input = Chunks::new(bytes.into_bytes(), vec![4096]);
    let (head, buffer, completed) = read_head(&mut input, start).await.unwrap();
    assert_eq!(completed, start);
    tokio::time::advance(BODY_TIMEOUT).await;
    assert_eq!(
        read_body(
            &mut input,
            buffer,
            head.content_length.unwrap(),
            completed + BODY_TIMEOUT
        )
        .await
        .err(),
        Some(RequestError::Reject(Code::RequestTimeout, false))
    );
}
