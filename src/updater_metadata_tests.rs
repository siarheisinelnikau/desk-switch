//! Real local HTTP responses exercise the production metadata request boundary.
//! No GitHub request, installation or application window is involved.
use super::*;
use std::{net::TcpListener, thread};

fn response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\nX-GitHub-Request-Id: local-fixture\r\n\r\n{body}",
        body.len()
    )
}

fn server(responses: Vec<String>) -> (reqwest::Url, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://{}/repos/owner/repository/releases/latest",
        listener.local_addr().unwrap()
    )
    .parse()
    .unwrap();
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let since = Instant::now();
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(accepted) => break accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(since.elapsed() < Duration::from_secs(3), "missing request");
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            // On Windows, accepted sockets inherit the nonblocking mode. Only
            // accept is polled; reading a request uses the bounded blocking API.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 16 * 1024);
                assert_eq!(stream.read(&mut byte).unwrap(), 1);
                request.push(byte[0]);
            }
            requests.push(String::from_utf8(request).unwrap());
            // Oversized-body tests may close after the bounded prefix.
            let _ = stream.write_all(response.as_bytes());
        }
        requests
    });
    (endpoint, worker)
}

fn fetch(endpoint: &reqwest::Url, pauses: &mut Vec<Duration>) -> Result<Option<Vec<u8>>> {
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .build()
        .unwrap();
    fetch_release_at(&client, endpoint, |duration| pauses.push(duration))
}

#[test]
fn transient_headers_retry_the_same_metadata_get_then_accept_one_body() {
    let (endpoint, server) = server(vec![
        response(504, "gateway"),
        response(503, "unavailable"),
        response(200, "verified later"),
    ]);
    let mut pauses = Vec::new();
    assert_eq!(
        fetch(&endpoint, &mut pauses).unwrap().unwrap(),
        b"verified later"
    );
    assert_eq!(
        pauses,
        [Duration::from_millis(250), Duration::from_millis(500)]
    );
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests {
        assert!(request.starts_with("GET /repos/owner/repository/releases/latest HTTP/1.1\r\n"));
        assert!(!request.to_ascii_lowercase().contains("authorization:"));
    }
}

#[test]
fn repeated_gateway_errors_stop_after_three_requests_with_endpoint_and_request_id() {
    let (endpoint, server) = server(vec![response(504, "gateway"); 3]);
    let error = format!("{:#}", fetch(&endpoint, &mut Vec::new()).unwrap_err());
    assert!(
        error.contains("HTTP 504") && error.contains("3 attempts"),
        "{error}"
    );
    assert!(
        error.contains(endpoint.as_str()) && error.contains("local-fixture"),
        "{error}"
    );
    assert_eq!(server.join().unwrap().len(), 3);
}

#[test]
fn unpublished_and_terminal_auth_or_rate_limit_responses_never_retry() {
    for status in [400, 401, 403, 404, 429] {
        let (endpoint, server) = server(vec![response(status, "terminal")]);
        let mut pauses = Vec::new();
        let result = fetch(&endpoint, &mut pauses);
        if status == 404 {
            assert!(result.unwrap().is_none());
        } else {
            assert!(format!("{:#}", result.unwrap_err()).contains(&format!("HTTP {status}")));
        }
        assert!(pauses.is_empty());
        assert_eq!(server.join().unwrap().len(), 1);
    }
}

#[test]
fn accepted_truncated_oversized_or_malformed_bodies_do_not_retry() {
    for raw in [
        "HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nshort".into(),
        response(200, &"x".repeat(MAX_MANIFEST as usize * 4 + 1)),
    ] {
        let (endpoint, server) = server(vec![raw]);
        let mut pauses = Vec::new();
        assert!(fetch(&endpoint, &mut pauses).is_err());
        assert!(pauses.is_empty());
        assert_eq!(server.join().unwrap().len(), 1);
    }
    let (endpoint, server) = server(vec![response(200, "invalid JSON")]);
    let mut pauses = Vec::new();
    let bytes = fetch(&endpoint, &mut pauses).unwrap().unwrap();
    assert!(release_envelope(&bytes).is_err());
    assert!(pauses.is_empty());
    assert_eq!(server.join().unwrap().len(), 1);
}
