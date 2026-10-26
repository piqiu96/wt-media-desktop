//! 命令测试共用的 HTTP 桩：固定应答，记下收到的每个请求。
//!
//! 应答按路径决定，所以「一条路由拒绝、另一条通过」是两个真实分支，而不是
//! 一个被 `?` 提前结束的序列。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// 一个监听循环，把每个请求的原文交给 `answer` 决定状态行与 body。
pub(crate) fn serving(
    answer: impl Fn(&str) -> (String, String) + Send + 'static,
) -> (u16, Arc<Mutex<Vec<String>>>) {
    let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = server.local_addr().expect("the bound address").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        for incoming in server.incoming() {
            let Ok(mut socket) = incoming else { break };
            let mut request = Vec::new();
            let mut chunk = [0u8; 1024];
            let mut complete = false;
            while let Ok(read) = socket.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        complete = true;
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&request).to_string();
            let path = text
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            sink.lock().expect("the recorder").push(text);
            if !complete {
                break;
            }
            let (status, body) = answer(&path);
            let response = format!(
                "HTTP/1.1 {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
            if socket.write_all(response.as_bytes()).is_err() {
                break;
            }
        }
    });
    (port, seen)
}

/// 一个没人监听的端口：先绑再放。
pub(crate) fn silent_port() -> u16 {
    let server = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    server.local_addr().expect("the bound address").port()
}

/// 收到的请求原文。
pub(crate) fn requests(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().expect("the recorder").clone()
}

/// 到达 `route` 的请求数。
pub(crate) fn arrivals(seen: &Arc<Mutex<Vec<String>>>, route: &str) -> usize {
    requests(seen)
        .iter()
        .filter(|request| request.split_whitespace().nth(1) == Some(route))
        .count()
}
