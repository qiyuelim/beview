//! 本地 OpenAI Responses API mock（系统边界，ADR-0016）：按请求 `stream` 标记分别回
//! SSE 事件流 / 普通 JSON。用例先 queue 期望的 content，取一个弹一个。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

type NonstreamQ = Arc<Mutex<Vec<String>>>;
type StreamQ = Arc<Mutex<Vec<Vec<String>>>>;
type ReqLog = Arc<Mutex<Vec<Value>>>;
/// 请求路径记录：锁定 URL 拼接正确性（必须命中 {base}/responses）
type PathLog = Arc<Mutex<Vec<String>>>;

/// 非流式响应前的延时（毫秒）：用于测试「任务 running 态可见/幂等去重/SSE 推送」
type DelayMs = Arc<AtomicU64>;
/// 流式响应 id 序号：生成 UUID 形态的 response.id（previous_response_id 链式断言用）
type StreamSeq = Arc<AtomicU64>;

pub struct LlmMock {
    pub port: u16,
    nonstream: NonstreamQ,
    stream: StreamQ,
    requests: ReqLog,
    paths: PathLog,
    delay_ms: DelayMs,
    stream_seq: StreamSeq,
    _task: tokio::task::JoinHandle<()>,
}

impl LlmMock {
    pub fn start() -> Arc<LlmMock> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let nonstream: NonstreamQ = Arc::new(Mutex::new(Vec::new()));
        let stream: StreamQ = Arc::new(Mutex::new(Vec::new()));
        let requests: ReqLog = Arc::new(Mutex::new(Vec::new()));
        let paths: PathLog = Arc::new(Mutex::new(Vec::new()));
        let delay_ms: DelayMs = Arc::new(AtomicU64::new(0));
        let stream_seq: StreamSeq = Arc::new(AtomicU64::new(0));
        let (n2, s2, r2, p2, d2, q2) = (nonstream.clone(), stream.clone(), requests.clone(), paths.clone(), delay_ms.clone(), stream_seq.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    continue;
                };
                let n = n2.clone();
                let s = s2.clone();
                let r = r2.clone();
                let p = p2.clone();
                let d = d2.clone();
                let q = q2.clone();
                tokio::spawn(async move {
                    handle_conn(&mut sock, &n, &s, &r, &p, &d, &q).await;
                });
            }
        });
        Arc::new(LlmMock {
            port,
            nonstream,
            stream,
            requests,
            paths,
            delay_ms,
            stream_seq,
            _task: task,
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// 排队一个非流式响应的 content（analyze/parse/paper 等）
    pub fn queue_nonstream(&self, content: &str) {
        self.nonstream.lock().unwrap().push(content.to_string());
    }

    /// 排队一个流式响应的 delta 序列
    pub fn queue_stream(&self, deltas: Vec<String>) {
        self.stream.lock().unwrap().push(deltas);
    }

    /// 已收到的全部请求体（断言传给 LLM 的上下文用）
    pub fn request_bodies(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// 已收到的全部请求路径（断言 URL 拼接：应为 {base_url}/responses）
    pub fn request_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }

    /// 设置非流式响应延时（毫秒，0 关闭）：模拟慢 LLM
    pub fn set_delay_ms(&self, ms: u64) {
        self.delay_ms.store(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

async fn handle_conn(
    sock: &mut TcpStream,
    nonstream: &NonstreamQ,
    stream: &StreamQ,
    requests: &ReqLog,
    paths: &PathLog,
    delay_ms: &DelayMs,
    stream_seq: &StreamSeq,
) {
    let mut buf = vec![0u8; 65536];
    let mut read = 0usize;
    loop {
        match sock.read(&mut buf[read..]).await {
            Ok(0) => return,
            Ok(n) => read += n,
            Err(_) => return,
        }
        if let Some(hi) = find(&buf[..read], b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..hi]);
            // 请求行（POST /v1/responses HTTP/1.1）：记录路径；严格网关模拟——
            // 非 /responses 结尾一律 405，锁定客户端 URL 拼接正确性
            let req_line = head.lines().next().unwrap_or("");
            let path = req_line.split_whitespace().nth(1).unwrap_or("").to_string();
            paths.lock().unwrap().push(path.clone());
            if !path.ends_with("/responses") {
                let body = json!({"error": {"message": "Method Not Allowed"}}).to_string();
                let rhead = format!(
                    "HTTP/1.1 405 Method Not Allowed\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(rhead.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
                return;
            }
            let mut len = 0usize;
            for line in head.lines() {
                if line.to_ascii_lowercase().starts_with("content-length:") {
                    len = line.split(':').nth(1).unwrap().trim().parse().unwrap_or(0);
                }
            }
            let body_start = hi + 4;
            while read < body_start + len {
                match sock.read(&mut buf[read..]).await {
                    Ok(0) => break,
                    Ok(n) => read += n,
                    Err(_) => return,
                }
            }
            let body = String::from_utf8_lossy(&buf[body_start..body_start + len]);
            let b: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            requests.lock().unwrap().push(b.clone());
            if b["stream"].as_bool().unwrap_or(false) {
                respond_stream(sock, stream, stream_seq).await;
            } else {
                let d = delay_ms.load(Ordering::SeqCst);
                if d > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(d)).await;
                }
                respond_json(sock, nonstream).await;
            }
            return;
        }
        if read >= buf.len() {
            return;
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

async fn respond_json(sock: &mut TcpStream, nonstream: &NonstreamQ) {
    // FIFO：queue_nonstream 按调用顺序入队、先入先出（与直觉一致；原 pop() 为 LIFO 易串序）
    let content = {
        let mut q = nonstream.lock().unwrap();
        if q.is_empty() {
            String::new()
        } else {
            q.remove(0)
        }
    };
    // Responses API 形态：output[].content[].text + usage(input/output_tokens)
    let resp = json!({
        "id": "resp_mock",
        "status": "completed",
        "output": [{
            "type": "message",
            "content": [{ "type": "output_text", "text": content }]
        }],
        "usage": { "input_tokens": 10, "output_tokens": 20 }
    });
    let payload = resp.to_string();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(payload.as_bytes()).await;
}

async fn respond_stream(sock: &mut TcpStream, stream: &StreamQ, seq: &AtomicU64) {
    // FIFO：queue_stream 先入先出
    let deltas = {
        let mut q = stream.lock().unwrap();
        if q.is_empty() {
            Vec::new()
        } else {
            q.remove(0)
        }
    };
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
    let _ = sock.write_all(head.as_bytes()).await;
    // 响应顶层 id（UUID 形态，与真实 Responses API 一致）：供 previous_response_id 链式断言。
    // 注意绝不是 output 数组里消息的 msg_* id。
    let n = seq.fetch_add(1, Ordering::SeqCst);
    let response_id = format!("f0dbb153-117f-9bbf-8176-5284b47f{:04x}", n);
    // Responses API 事件序列：created → output_text.delta × N → completed(含 usage + 响应 id)
    let created = format!(
        "data: {}\n\n",
        json!({ "type": "response.created", "response": { "id": response_id } }).to_string()
    );
    let _ = sock.write_all(created.as_bytes()).await;
    for d in deltas {
        let line = format!(
            "data: {}\n\n",
            json!({ "type": "response.output_text.delta", "delta": d }).to_string()
        );
        let _ = sock.write_all(line.as_bytes()).await;
    }
    let done = format!(
        "data: {}\n\n",
        json!({
            "type": "response.completed",
            "response": { "id": response_id, "usage": { "input_tokens": 10, "output_tokens": 20 } }
        })
        .to_string()
    );
    let _ = sock.write_all(done.as_bytes()).await;
}

impl LlmMock {
    /// 诊断用：非流式队列剩余长度
    pub fn queue_nonstream_len(&self) -> usize {
        self.nonstream.lock().unwrap().len()
    }
}
