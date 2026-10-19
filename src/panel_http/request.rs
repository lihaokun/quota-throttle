use super::{HttpErrorCode as Code, StartupError};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::{timeout_at, Instant};

pub(super) const MAX_HEAD: usize = 16 * 1024;
pub(super) const MAX_TARGET: usize = 8 * 1024;
pub(super) const MAX_BODY: usize = 1024 * 1024;
const MAX_FIELDS: usize = 64;
const CHUNK: usize = 4096;
const HEAD_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const BODY_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) struct PanelOrigin {
    pub(super) origin: String,
    pub(super) authority: String,
}
impl PanelOrigin {
    pub(super) fn parse(text: &str) -> Result<Self, StartupError> {
        let bad = StartupError::InvalidOrigin;
        let (scheme, rest) = text.split_once("://").ok_or(bad)?;
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if !scheme.eq_ignore_ascii_case("http") || !valid_authority(authority) {
            return Err(bad);
        }
        let url = reqwest::Url::parse(text).map_err(|_| bad)?;
        if !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            || url.port_or_known_default().is_none_or(|p| p == 0)
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(bad);
        }
        let origin = url.origin().ascii_serialization();
        let authority = origin.strip_prefix("http://").ok_or(bad)?.to_owned();
        Ok(Self { origin, authority })
    }
}
fn valid_authority(value: &str) -> bool {
    if value.is_empty()
        || value
            .bytes()
            .any(|c| !(0x21..=0x7e).contains(&c) || b"/@?#\\,".contains(&c))
    {
        return false;
    }
    reqwest::Url::parse(&format!("http://{value}/")).is_ok_and(|u| u.has_host())
}
pub(super) struct RequestHead {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) query: Option<String>,
    headers: BTreeMap<String, String>,
    pub(super) content_length: Option<usize>,
}
impl RequestHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum RequestError {
    Reject(Code, bool),
    Closed,
}
fn token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c)
}
fn writing(method: &str) -> bool {
    matches!(method, "POST" | "PUT" | "DELETE")
}
fn ows(s: &str) -> &str {
    s.trim_matches([' ', '\t'])
}

pub(super) fn parse_head(bytes: &[u8]) -> Result<RequestHead, Code> {
    if bytes.len() > MAX_HEAD {
        return Err(Code::HeaderTooLarge);
    }
    if !bytes.is_ascii() {
        return Err(Code::BadRequest);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Code::BadRequest)?;
    let text = text.strip_suffix("\r\n\r\n").ok_or(Code::BadRequest)?;
    let mut lines = text.split("\r\n");
    let mut start = lines.next().ok_or(Code::BadRequest)?.split(' ');
    let method = start.next().ok_or(Code::BadRequest)?;
    let target = start.next().ok_or(Code::BadRequest)?;
    let version = start.next().ok_or(Code::BadRequest)?;
    if start.next().is_some() || method.is_empty() || !method.bytes().all(token) {
        return Err(Code::BadRequest);
    }
    if target.len() > MAX_TARGET {
        return Err(Code::TargetTooLong);
    }
    if target.is_empty()
        || target
            .bytes()
            .any(|c| !(0x21..=0x7e).contains(&c) || c == b'#')
    {
        return Err(Code::BadRequest);
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q.to_owned())),
        None => (target, None),
    };
    if !path.starts_with('/')
        || path.contains("//")
        || path.split('/').any(|p| p == "." || p == "..")
        || !path
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
    {
        return Err(Code::BadRequest);
    }
    let v = version.as_bytes();
    if v.len() != 8
        || &v[..5] != b"HTTP/"
        || !v[5].is_ascii_digit()
        || v[6] != b'.'
        || !v[7].is_ascii_digit()
    {
        return Err(Code::BadRequest);
    }
    if version != "HTTP/1.1" {
        return Err(Code::UnsupportedVersion);
    }
    let mut headers = BTreeMap::new();
    for (n, line) in lines.enumerate() {
        if n >= MAX_FIELDS {
            return Err(Code::HeaderTooLarge);
        }
        let (name, value) = line.split_once(':').ok_or(Code::BadRequest)?;
        if name.is_empty()
            || !name.bytes().all(token)
            || !value
                .bytes()
                .all(|c| c == b'\t' || (0x20..=0x7e).contains(&c))
        {
            return Err(Code::BadRequest);
        }
        if headers
            .insert(name.to_ascii_lowercase(), ows(value).to_owned())
            .is_some()
        {
            return Err(Code::BadRequest);
        }
    }
    let head = RequestHead {
        method: method.to_owned(),
        path: path.to_owned(),
        query,
        content_length: None,
        headers,
    };
    if !head.header("host").is_some_and(valid_authority) {
        return Err(Code::BadRequest);
    }
    if head.header("transfer-encoding").is_some()
        || head.header("upgrade").is_some()
        || head
            .header("connection")
            .is_some_and(|s| s.split(',').any(|v| ows(v).eq_ignore_ascii_case("upgrade")))
    {
        return Err(Code::BadRequest);
    }
    if head.header("content-encoding").is_some() {
        return Err(Code::UnsupportedMediaType);
    }
    if head.header("expect").is_some() {
        return Err(Code::ExpectationFailed);
    }
    let content_length = match head.header("content-length") {
        None => None,
        Some(value) => {
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(Code::BadRequest);
            }
            let n = value
                .bytes()
                .try_fold(0u64, |n, c| {
                    n.checked_mul(10)?.checked_add(u64::from(c - b'0'))
                })
                .ok_or(Code::BadRequest)?;
            if n > MAX_BODY as u64 {
                return Err(Code::BodyTooLarge);
            }
            Some(n as usize)
        }
    };
    if matches!(method, "GET" | "HEAD") && content_length.is_some_and(|n| n != 0) {
        return Err(Code::BadRequest);
    }
    if writing(method) && content_length.is_none() {
        return Err(Code::LengthRequired);
    }
    Ok(RequestHead {
        content_length,
        ..head
    })
}

pub(super) async fn read_head<R: AsyncRead + Unpin>(
    stream: &mut R,
    accepted_at: Instant,
) -> Result<(RequestHead, Vec<u8>, Instant), RequestError> {
    // Step H1.2: 字节分帧与终止位置双重限额；每次只扫描新增字节及3字节重叠。
    let mut buf = Vec::with_capacity(MAX_HEAD + CHUNK);
    let mut scan_from = 0;
    let mut chunk = [0u8; CHUNK];
    let deadline = accepted_at + HEAD_TIMEOUT;
    loop {
        let head_only = buf.starts_with(b"HEAD ");
        check_deadline(deadline, head_only)?;
        if let Some(pos) = buf[scan_from..].windows(4).position(|w| w == b"\r\n\r\n") {
            let completed_at = Instant::now();
            let end = scan_from + pos + 4;
            if end > MAX_HEAD {
                return Err(RequestError::Reject(Code::HeaderTooLarge, head_only));
            }
            let head = parse_head(&buf[..end]).map_err(|c| RequestError::Reject(c, head_only))?;
            check_deadline(deadline, head_only)?;
            return Ok((head, buf[end..].to_vec(), completed_at));
        }
        if buf.len() >= MAX_HEAD {
            return Err(RequestError::Reject(Code::HeaderTooLarge, head_only));
        }
        scan_from = buf.len().saturating_sub(3);
        check_deadline(deadline, head_only)?;
        let n = timeout_at(deadline, stream.read(&mut chunk))
            .await
            .map_err(|_| RequestError::Reject(Code::RequestTimeout, head_only))?
            .map_err(|_| RequestError::Closed)?;
        if n == 0 {
            check_deadline(deadline, head_only)?;
            return Err(RequestError::Reject(Code::BadRequest, head_only));
        }
        buf.extend_from_slice(&chunk[..n]);
        check_deadline(deadline, buf.starts_with(b"HEAD "))?;
    }
}
fn check_deadline(deadline: Instant, head_only: bool) -> Result<(), RequestError> {
    if Instant::now() >= deadline {
        Err(RequestError::Reject(Code::RequestTimeout, head_only))
    } else {
        Ok(())
    }
}
pub(super) fn authorize(head: &RequestHead, origin: &PanelOrigin) -> Result<(), Code> {
    // Step H1.3: 固定Host/Origin和非简单请求头，不把它们当作本机用户身份认证。
    if head.header("host") != Some(origin.authority.as_str()) {
        return Err(Code::Forbidden);
    }
    if head.header("origin").is_some_and(|v| v != origin.origin) {
        return Err(Code::Forbidden);
    }
    if head.path.starts_with("/api/admin/v1/") && head.header("x-qt-panel") != Some("1") {
        return Err(Code::Forbidden);
    }
    if head
        .header("sec-fetch-site")
        .is_some_and(|v| !matches!(v, "same-origin" | "none"))
    {
        return Err(Code::Forbidden);
    }
    if writing(&head.method) {
        if head.header("origin") != Some(origin.origin.as_str()) {
            return Err(Code::Forbidden);
        }
        let mut parts = head
            .header("content-type")
            .ok_or(Code::UnsupportedMediaType)?
            .split(';');
        if !ows(parts.next().unwrap_or("")).eq_ignore_ascii_case("application/json") {
            return Err(Code::UnsupportedMediaType);
        }
        if parts
            .next()
            .is_some_and(|p| !ows(p).eq_ignore_ascii_case("charset=utf-8"))
            || parts.next().is_some()
        {
            return Err(Code::UnsupportedMediaType);
        }
    }
    Ok(())
}
pub(super) async fn read_body<R: AsyncRead + Unpin>(
    stream: &mut R,
    buffered: Vec<u8>,
    length: usize,
    deadline: Instant,
) -> Result<Vec<u8>, RequestError> {
    if length > MAX_BODY {
        return Err(RequestError::Reject(Code::BodyTooLarge, false));
    }
    check_deadline(deadline, false)?;
    let mut body = Vec::with_capacity(length);
    body.extend_from_slice(&buffered[..buffered.len().min(length)]);
    let mut chunk = [0u8; CHUNK];
    while body.len() < length {
        check_deadline(deadline, false)?;
        let wanted = (length - body.len()).min(CHUNK);
        let n = timeout_at(deadline, stream.read(&mut chunk[..wanted]))
            .await
            .map_err(|_| RequestError::Reject(Code::RequestTimeout, false))?
            .map_err(|_| RequestError::Closed)?;
        check_deadline(deadline, false)?;
        if n == 0 {
            return Err(RequestError::Reject(Code::BadRequest, false));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    std::str::from_utf8(&body).map_err(|_| RequestError::Reject(Code::BadRequest, false))?;
    check_deadline(deadline, false)?;
    Ok(body)
}

#[cfg(test)]
#[path = "tests/request.rs"]
mod tests;
