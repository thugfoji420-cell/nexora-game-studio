//! Shared mock HTTP server utilities for the test suites.
//!
//! Every test module used to carry its own loopback HTTP stub — request
//! parsing, canned responses and capture plumbing. That plumbing lives here
//! once: [`MockHttpServer`] accepts a bounded number of connections on
//! `127.0.0.1`, parses each request into a [`MockRequest`], asks a handler
//! for the raw response to write, and captures the requests so tests can
//! assert on them afterwards.
//!
//! This crate is a `dev-dependency` of the application and must never be
//! linked into the shipped binary. It is deliberately std-only.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long the server waits for the next connection before failing: a
/// client that stops reaching the mock endpoint is a real test failure, not
/// something to hang on.
const ACCEPT_DEADLINE: Duration = Duration::from_secs(15);

/// One captured HTTP request: raw bytes plus small helpers so tests assert
/// on the request line, headers and body without re-implementing HTTP.
#[derive(Debug)]
pub struct MockRequest {
    raw: Vec<u8>,
}

impl MockRequest {
    /// Request line + headers — everything before the blank line.
    pub fn head(&self) -> &str {
        let end = self.header_end();
        std::str::from_utf8(&self.raw[..end]).expect("captured request head is not UTF-8")
    }

    /// First line, e.g. `GET /v1/models HTTP/1.1`.
    pub fn request_line(&self) -> &str {
        self.head().lines().next().unwrap_or_default()
    }

    /// Request body (everything after the blank line).
    pub fn body(&self) -> &[u8] {
        &self.raw[self.header_end() + 4..]
    }

    /// Request body as UTF-8 (lossy) — string-level assertions read this.
    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(self.body()).into_owned()
    }

    /// Case-insensitive request header lookup.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.head().lines().skip(1).find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    /// Raw request bytes (request line + headers + body).
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    fn header_end(&self) -> usize {
        self.raw
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
            .expect("captured request has no header terminator")
    }
}

/// Build a raw canned response: `status_line`, optional extra `headers`,
/// then `Content-Length`/`Connection: close` and the body.
pub fn response(status_line: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut bytes = format!("{status_line}\r\n").into_bytes();
    for (name, value) in headers {
        bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes(),
    );
    bytes.extend_from_slice(body);
    bytes
}

/// `200 OK` carrying the body verbatim (no Content-Type), the shape every
/// provider stub in the suites answers with.
pub fn ok_response(body: &[u8]) -> Vec<u8> {
    response("HTTP/1.1 200 OK", &[], body)
}

/// JSON response with an explicit status line reason (e.g. `429 Too Many
/// Requests` for error stubs).
pub fn json_response(status: u16, reason: &str, body: &str) -> Vec<u8> {
    response(
        &format!("HTTP/1.1 {status} {reason}"),
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

/// A bounded mock HTTP server on `127.0.0.1`. Serves exactly as many
/// exchanges as configured, so no dead listener outlives the test.
pub struct MockHttpServer {
    base_url: String,
    requests: Receiver<MockRequest>,
    handle: Option<JoinHandle<()>>,
}

impl MockHttpServer {
    /// Replay `responses`: one canned raw response per connection, in order.
    pub fn serve(responses: Vec<Vec<u8>>) -> Self {
        let total = responses.len();
        Self::serve_with(total, move |index, _request| responses[index].clone())
    }

    /// Serve exactly `count` exchanges. `handler` builds the raw response for
    /// the `index`-th (0-based) captured request; assertions inside the
    /// handler run on the server thread and surface when the server is
    /// joined via [`Self::requests`] / [`Self::join`].
    pub fn serve_with(
        count: usize,
        handler: impl FnMut(usize, &MockRequest) -> Vec<u8> + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback mock port");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel();
        let handle = thread::spawn(move || {
            let mut handler = handler;
            for index in 0..count {
                let mut stream = accept_with_deadline(&listener, ACCEPT_DEADLINE);
                // Accepted sockets inherit the listener's non-blocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let request = MockRequest {
                    raw: read_request(&mut stream),
                };
                let response = handler(index, &request);
                // Capture is best-effort: dropping the server must not stop
                // the remaining canned responses from being served.
                let _ = sender.send(request);
                stream.write_all(&response).unwrap();
            }
        });
        Self {
            base_url,
            requests,
            handle: Some(handle),
        }
    }

    /// Loopback base URL without a trailing slash, e.g.
    /// `http://127.0.0.1:53211`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Block until the server has captured the next request.
    pub fn next_request(&self) -> MockRequest {
        self.requests.recv().expect("mock server thread ended early")
    }

    /// Wait for the server to finish and return every captured request in
    /// order. Handler assertions and accept-deadline failures propagate.
    pub fn requests(mut self) -> Vec<MockRequest> {
        self.join_thread();
        self.requests.into_iter().collect()
    }

    /// Wait for the server thread to finish, propagating handler panics.
    pub fn join(mut self) {
        self.join_thread();
    }

    fn join_thread(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn accept_with_deadline(listener: &TcpListener, deadline: Duration) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < deadline,
                    "client never reached the mock endpoint"
                );
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("mock endpoint accept failed: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).expect("mock request read failed");
        assert!(count > 0, "connection closed before a full request arrived");
        request.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = request
            .windows(4)
            .position(|value| value == b"\r\n\r\n")
        {
            let content_length = String::from_utf8_lossy(&request[..header_end])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>())
                        .and_then(Result::ok)
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                return request;
            }
        }
    }
}
