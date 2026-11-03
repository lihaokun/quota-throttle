use super::*;
use crate::panel_http::{
    admin::tests::support::{fixture, Running},
    PanelServer,
};

#[test]
fn u1_public_root_validation_and_canonical_byte_limit() {
    for raw in [
        "",
        " http://example.test",
        "http://example.test ",
        "ftp://example.test",
        "https://u:p@example.test",
        "https://@example.test",
        "https://example.test/?",
        "https://example.test/#",
        "https://example.test/v1",
        "https://example.test/V1/",
        "https://example.test/%76%31///",
        "https://example.test/%56%31",
        "https://example.test/%zz",
        "https://example.test/%zz/proxy",
        "https://example.test/%/proxy",
        "https://example.test/a|b/proxy",
        "https://example.test/a^b/proxy",
        "https://example.test/[x]/proxy",
        "https://example.test/\nfoo",
        "https://example.test\\foo",
    ] {
        assert!(
            matches!(
                UiOptions::new(Some(raw.to_owned())),
                Err(StartupError::InvalidUiConfig)
            ),
            "must reject {raw:?}"
        );
    }
    for (raw, expected) in [
        ("HTTP://EXAMPLE.TEST:80///", "http://example.test"),
        (
            "https://example.test/proxy///",
            "https://example.test/proxy",
        ),
        ("https://example.test/v10", "https://example.test/v10"),
        (
            "https://example.test/%25/proxy",
            "https://example.test/%25/proxy",
        ),
        ("http://[::1]:1234", "http://[::1]:1234"),
    ] {
        assert_eq!(
            UiOptions::new(Some(raw.into()))
                .unwrap()
                .public_api_base_url
                .as_deref(),
            Some(expected)
        );
    }
    let prefix = "https://example.test/";
    assert!(UiOptions::new(Some(format!(
        "{prefix}{}",
        "a".repeat(MAX_URL - prefix.len())
    )))
    .is_ok());
    assert!(UiOptions::new(Some(format!(
        "{prefix}{}",
        "a".repeat(MAX_URL - prefix.len() + 1)
    )))
    .is_err());
    let expands = format!("{prefix}{}", "中".repeat(1000));
    assert!(expands.len() < MAX_URL);
    assert!(UiOptions::new(Some(expands)).is_err());
    assert!(UiOptions::new(None).unwrap().public_api_base_url.is_none());
}

#[test]
fn u1_compiled_resource_budget_and_shared_bytes() {
    let bundle = UiBundle::new(UiOptions::default(), false).unwrap();
    assert!(ASSETS.iter().all(|(_, _, bytes)| bytes.len() <= 512 * 1024));
    assert!(
        ASSETS
            .iter()
            .map(|(_, _, bytes)| bytes.len())
            .sum::<usize>()
            <= 1024 * 1024
    );
    for (path, _, _) in ASSETS {
        let head = super::super::request::parse_head(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
        )
        .unwrap();
        let a = bundle.response(&head).unwrap();
        let b = bundle.response(&head).unwrap();
        assert!(Arc::ptr_eq(&a.body, &b.body));
    }
}

async fn start(
    dry_run: bool,
    root: Option<&str>,
) -> (crate::panel_http::admin::tests::support::Fixture, Running) {
    let fixture = fixture(dry_run);
    let server = PanelServer::bind("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let addr = server.listener.local_addr().unwrap();
    assert!(![3000, 3001].contains(&addr.port()));
    let authority = server.origin.authority.clone();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(server.serve_ui(
        fixture.worker.clone(),
        UiOptions::new(root.map(str::to_owned)).unwrap(),
        shutdown,
    ));
    (
        fixture,
        Running {
            addr,
            authority,
            stop,
            task,
        },
    )
}
fn wire(server: &Running, method: &str, path: &str, extra: &str) -> Vec<u8> {
    format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\n{extra}\r\n",
        server.authority
    )
    .into_bytes()
}

#[tokio::test]
async fn u1_static_get_head_mime_csp_and_strict_whitelist() {
    let (_fixture, server) = start(false, None).await;
    for (path, content_type, original) in ASSETS {
        let (status, header, bytes) = server.raw(&wire(&server, "GET", path, "")).await;
        assert_eq!(status, 200, "{path}");
        assert_eq!(bytes, *original);
        assert!(header.contains(&format!("Content-Type: {}\r\n", content_type.header())));
        assert_eq!(
            header.contains(CSP_HEADER),
            *content_type == ContentType::Html
        );
        assert!(header.contains("Cross-Origin-Resource-Policy: same-origin\r\n"));
        assert!(header.contains("Referrer-Policy: no-referrer\r\n"));
        assert!(header.contains("X-Frame-Options: DENY\r\n"));
        let (status, head, body) = server.raw(&wire(&server, "HEAD", path, "")).await;
        assert_eq!(status, 200);
        assert_eq!(head, header);
        assert!(body.is_empty());
        for suffix in ["?", "?x=1"] {
            assert_eq!(
                server
                    .raw(&wire(&server, "GET", &format!("{path}{suffix}"), ""))
                    .await
                    .0,
                400
            );
        }
        assert_eq!(
            server
                .raw(&wire(
                    &server,
                    "POST",
                    path,
                    "Content-Type: application/json\r\n"
                ))
                .await
                .0,
            403
        );
        let (status, header, _) = server
            .raw(&wire(
                &server,
                "POST",
                path,
                &format!(
                    "Origin: http://{}\r\nContent-Type: application/json\r\n",
                    server.authority
                ),
            ))
            .await;
        assert_eq!(status, 405);
        assert!(header.contains("Allow: GET, HEAD\r\n"));
    }
    for path in [
        "/index.html",
        "/assets/absent.mjs",
        "/assets/",
        "/some-page",
    ] {
        assert_eq!(server.raw(&wire(&server, "GET", path, "")).await.0, 404);
    }
    server.finish().await;
}

#[tokio::test]
async fn u1_ui_config_same_worker_default_explicit_and_no_head_business() {
    for (dry_run, root) in [
        (false, None),
        (true, Some("https://public.example.test/proxy/")),
    ] {
        let (_fixture, server) = start(dry_run, root).await;
        let (status, header, bytes) = server
            .raw(&wire(&server, "GET", CONFIG_PATH, "X-QT-Panel: 1\r\n"))
            .await;
        assert_eq!(status, 200);
        assert!(header.contains("Content-Type: application/json; charset=utf-8\r\n"));
        assert!(!header.contains("Content-Security-Policy:"));
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"schema_version": 1, "dry_run": dry_run, "public_api_base_url": root.map(|r| r.trim_end_matches('/'))})
        );
        assert!(bytes.len() <= 16 * 1024);
        let (status, header, bytes) = server
            .raw(&wire(&server, "HEAD", CONFIG_PATH, "X-QT-Panel: 1\r\n"))
            .await;
        assert_eq!(status, 405);
        assert!(header.contains("Allow: GET\r\n"));
        assert!(bytes.is_empty());
        assert_eq!(
            server
                .raw(&wire(
                    &server,
                    "GET",
                    &format!("{CONFIG_PATH}?"),
                    "X-QT-Panel: 1\r\n"
                ))
                .await
                .0,
            400
        );
        server.finish().await;
    }
}

#[tokio::test]
async fn u1_static_does_not_weaken_api_or_origin_authorization() {
    let (_fixture, server) = start(false, None).await;
    for path in [CONFIG_PATH, "/api/admin/v1/state", "/api/admin/v1/tokens"] {
        assert_eq!(server.raw(&wire(&server, "GET", path, "")).await.0, 403);
    }
    for path in ["/", "/assets/app.mjs", CONFIG_PATH] {
        for headers in [
            "Origin: https://evil.invalid\r\nX-QT-Panel: 1\r\n",
            "Sec-Fetch-Site: cross-site\r\nX-QT-Panel: 1\r\n",
        ] {
            assert_eq!(
                server.raw(&wire(&server, "GET", path, headers)).await.0,
                403
            );
        }
        let bad_host =
            format!("GET {path} HTTP/1.1\r\nHost: evil.invalid\r\nX-QT-Panel: 1\r\n\r\n");
        assert_eq!(server.raw(bad_host.as_bytes()).await.0, 403);
    }
    server.finish().await;
}

// 这是显式启动的浏览器夹具进程，不是跳过的正确性或性能断言。
#[tokio::test]
#[ignore = "isolated browser fixture server, explicitly run by UI browser runner"]
async fn u10_browser_fixture_server() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("QT_BROWSER_FIXTURE_DIR").expect("explicit fixture directory"),
    );
    let directory = directory.canonicalize().unwrap();
    assert!(directory.starts_with("/tmp"));
    assert!(directory
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("quota-throttle-sgm6-b-browser-"));
    let (_fixture, server) = start(false, Some("https://public.example.test/proxy")).await;
    std::fs::write(
        directory.join("address"),
        format!("http://{}", server.authority),
    )
    .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3600);
    loop {
        if directory.join("stop").exists() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "browser fixture exceeded maximum lifetime"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    server.finish().await;
}
