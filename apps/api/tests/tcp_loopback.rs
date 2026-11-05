//! End-to-end TCP socket integration test for the `lora-api` loopback server (`docs/API_AND_SDK.md`, `R08`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

fn find_free_loopback_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    format!("127.0.0.1:{}", addr.port())
}

#[test]
fn loopback_api_live_tcp_socket_e2e() {
    let api_bin = env!("CARGO_BIN_EXE_lora-api");
    let tmp = TempDir::new().expect("tempdir");

    // 1. Verify --print-openapi outputs valid OpenAPI 3.1 JSON
    let openapi_out = Command::new(api_bin)
        .arg("--print-openapi")
        .output()
        .expect("run --print-openapi");
    assert!(openapi_out.status.success());
    let openapi_str = String::from_utf8_lossy(&openapi_out.stdout);
    assert!(openapi_str.contains("\"openapi\": \"3.1.0\""));
    assert!(openapi_str.contains("\"/v1/capabilities\""));

    // 2. Spawn live `lora-api --once` on loopback port and send authenticated GET /v1/capabilities
    let bind_addr = find_free_loopback_addr();
    let mut child = Command::new(api_bin)
        .args([
            "--data-dir",
            tmp.path().to_str().unwrap(),
            "--bind",
            &bind_addr,
            "--token",
            "live-secret-token-42",
            "--once",
        ])
        .spawn()
        .expect("spawn lora-api");

    // Connect with retry while child binds socket
    let mut stream = None;
    for _ in 0..40 {
        if let Ok(s) = TcpStream::connect(&bind_addr) {
            stream = Some(s);
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let mut stream = stream.expect("connect to live lora-api");
    let req = format!(
        "GET /v1/capabilities HTTP/1.1\r\nHost: {bind_addr}\r\nAuthorization: Bearer live-secret-token-42\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).unwrap();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(resp.starts_with("HTTP/1.1 200 OK"));
    assert!(resp.contains("\"api_major\":1"));
    assert!(resp.contains("\"runs.events.sse\""));
}
