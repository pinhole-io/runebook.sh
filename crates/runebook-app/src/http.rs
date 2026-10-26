//! HTTP client for a remote image in a note.
//!
//! GPUI loads an `http` or `https` image through the application client.
//! The default client refuses every request, so the picture never appears.
//! This client follows redirects and stops at 8 MiB.
//! A note can name any `http` or `https` address. A failure shows the alt text.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui_kit::http_client::{self, AsyncBody, HttpClient, Inner, RedirectPolicy, RequestTimeout};

const MAX_BODY: u64 = 8 * 1024 * 1024;
const USER_AGENT: &str = "runebook";

struct RemoteClient {
    follow: reqwest::Client,
    plain: reqwest::Client,
    user_agent: http_client::http::HeaderValue,
}

impl RemoteClient {
    fn new() -> Self {
        Self {
            follow: build(reqwest::redirect::Policy::limited(10)),
            plain: build(reqwest::redirect::Policy::none()),
            user_agent: http_client::http::HeaderValue::from_static(USER_AGENT),
        }
    }
}

/// Client the window uses for a remote image.
pub(crate) fn client() -> Arc<dyn HttpClient> {
    Arc::new(RemoteClient::new())
}

fn build(policy: reqwest::redirect::Policy) -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(policy)
        .build()
        .expect("the image client uses a fixed configuration")
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("runebook-http")
            .enable_all()
            .build()
            .expect("http runtime")
    })
}

impl HttpClient for RemoteClient {
    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&http_client::Url> {
        None
    }

    fn send(
        &self,
        req: http_client::Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, http_client::Result<http_client::Response<AsyncBody>>>
    {
        let follow = self.follow.clone();
        let plain = self.plain.clone();
        Box::pin(async move {
            runtime()
                .spawn(async move { fetch(follow, plain, req, MAX_BODY).await })
                .await
                .map_err(|_| http_client::anyhow!("the image request failed"))?
        })
    }
}

async fn fetch(
    follow: reqwest::Client,
    plain: reqwest::Client,
    req: http_client::Request<AsyncBody>,
    max: u64,
) -> http_client::Result<http_client::Response<AsyncBody>> {
    let (parts, body) = req.into_parts();
    if !scheme_allowed(parts.uri.scheme_str()) {
        return Err(http_client::anyhow!(
            "the image address is not http or https",
        ));
    }
    let follow_redirects = !matches!(
        parts.extensions.get::<RedirectPolicy>(),
        Some(RedirectPolicy::NoFollow)
    );
    let client = if follow_redirects { &follow } else { &plain };
    let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
        .map_err(|_| http_client::anyhow!("the image request failed"))?;
    let mut builder = client.request(method, parts.uri.to_string());
    builder = builder.headers(parts.headers);
    if let Some(timeout) = parts.extensions.get::<RequestTimeout>() {
        builder = builder.timeout(timeout.0);
    }
    let reqwest_body = match body.0 {
        Inner::Empty => reqwest::Body::default(),
        Inner::Bytes(cursor) => reqwest::Body::from(cursor.into_inner()),
        Inner::AsyncReader(_) => {
            return Err(http_client::anyhow!(
                "a streamed image request is not supported",
            ));
        }
    };
    let response = builder
        .body(reqwest_body)
        .send()
        .await
        .map_err(|_| http_client::anyhow!("the image request failed"))?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let bytes = read_capped(response, max).await?;
    let mut builder = http_client::http::Response::builder().status(status);
    *builder
        .headers_mut()
        .ok_or_else(|| http_client::anyhow!("the image request failed"))? = headers;
    builder
        .body(AsyncBody::from(bytes))
        .map_err(|_| http_client::anyhow!("the image request failed"))
}

fn scheme_allowed(scheme: Option<&str>) -> bool {
    matches!(scheme, Some("http" | "https"))
}

async fn read_capped(mut response: reqwest::Response, max: u64) -> http_client::Result<Vec<u8>> {
    if response.content_length().is_some_and(|len| len > max) {
        return Err(http_client::anyhow!("the image is too large"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| http_client::anyhow!("the image request failed"))?
    {
        let next = body.len() as u64 + chunk.len() as u64;
        if next > max {
            return Err(http_client::anyhow!("the image is too large"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use gpui_kit::http_client::HttpRequestExt;

    use super::*;

    #[test]
    fn only_http_and_https_are_image_addresses() {
        assert!(scheme_allowed(Some("http")));
        assert!(scheme_allowed(Some("https")));
        assert!(!scheme_allowed(Some("file")));
        assert!(!scheme_allowed(Some("data")));
        assert!(!scheme_allowed(None));
    }

    #[test]
    fn a_remote_image_returns_the_body() {
        let url = serve("200 OK", &[], b"png-bytes");
        let response = wait(url, None).expect("image");
        assert!(response.status().is_success());
        assert_eq!(body_bytes(response), b"png-bytes");
    }

    #[test]
    fn a_redirect_reaches_the_image() {
        let image = serve("200 OK", &[], b"mountain");
        let redirect = serve("302 Found", &[("Location", image.as_str())], b"");
        let response = wait(redirect, None).expect("image");
        assert!(response.status().is_success());
        assert_eq!(body_bytes(response), b"mountain");
    }

    #[test]
    fn a_request_can_refuse_a_redirect() {
        let image = serve("200 OK", &[], b"mountain");
        let redirect = serve("302 Found", &[("Location", image.as_str())], b"");
        let response = wait(redirect, Some(RedirectPolicy::NoFollow)).expect("redirect");
        assert_eq!(response.status().as_u16(), 302);
    }

    #[test]
    fn a_large_image_is_refused() {
        let url = serve("200 OK", &[], b"0123456789");
        let request = http_client::Request::get(&url)
            .body(AsyncBody::empty())
            .expect("request");
        let client = RemoteClient::new();
        let error =
            runtime().block_on(async move { fetch(client.follow, client.plain, request, 4).await });
        match error {
            Ok(_) => panic!("a large image must fail"),
            Err(error) => assert_eq!(error.to_string(), "the image is too large"),
        }
    }

    #[test]
    fn a_non_http_address_does_not_load() {
        let request = http_client::Request::get("ftp://example.com/picture.png")
            .body(AsyncBody::empty())
            .expect("request");
        match wait_request(request) {
            Ok(_) => panic!("a non-http address must fail"),
            Err(error) => {
                assert_eq!(error.to_string(), "the image address is not http or https");
            }
        }
    }

    fn wait(
        url: String,
        policy: Option<RedirectPolicy>,
    ) -> http_client::Result<http_client::Response<AsyncBody>> {
        let mut builder = http_client::Request::get(&url);
        if let Some(policy) = policy {
            builder = builder.follow_redirects(policy);
        }
        let request = builder.body(AsyncBody::empty()).expect("request");
        wait_request(request)
    }

    fn wait_request(
        request: http_client::Request<AsyncBody>,
    ) -> http_client::Result<http_client::Response<AsyncBody>> {
        let client = RemoteClient::new();
        runtime().block_on(async move { client.send(request).await })
    }

    fn body_bytes(response: http_client::Response<AsyncBody>) -> Vec<u8> {
        let AsyncBody(inner) = response.into_body();
        match inner {
            Inner::Bytes(cursor) => cursor.into_inner().to_vec(),
            Inner::Empty => Vec::new(),
            Inner::AsyncReader(_) => panic!("the image body is in memory"),
        }
    }

    fn serve(status: &str, headers: &[(&str, &str)], body: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let status = status.to_string();
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let body = body.to_vec();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
            for (name, value) in headers {
                head.push_str(&name);
                head.push_str(": ");
                head.push_str(&value);
                head.push_str("\r\n");
            }
            head.push_str("Connection: close\r\n\r\n");
            stream.write_all(head.as_bytes()).expect("write head");
            stream.write_all(&body).expect("write body");
        });
        format!("http://{address}/picture.png")
    }
}
