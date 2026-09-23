//! Terminal rich-text projection and bounded, anonymous image loading.
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::Arc;

use bmux_tui::image::{ImagePayload, ImagePixelFormat};
use bmux_tui::prelude::{Line, Modifier, Span, Style};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub full: ImagePayload,
    pub preview: ImagePayload,
}

pub type Images = BTreeMap<String, Result<Arc<DecodedImage>, ImageError>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    UnsafeUrl,
    Network,
    AccessDenied,
    NotFound,
    TooLarge,
    Decode,
    Timeout,
    Dns,
    Http(u16),
    Response {
        status: u16,
        kind: &'static str,
        bytes: usize,
        format: &'static str,
        reason: &'static str,
    },
    Redirect,
    Limit,
}

impl ImageError {
    pub fn details(self) -> String {
        match self {
            Self::Response {
                status,
                kind,
                bytes,
                format,
                reason,
            } => format!(
                "{reason}\nHTTP: {status}\nContent type: {kind}\nReceived: {bytes} bytes\nDetected format: {format}\nLimits: 16 MiB download; 4096 pixels per dimension; 32 MiB decode memory.\nOpen externally to inspect the original."
            ),
            Self::Http(status) => format!("Image download failed: HTTP {status}."),
            _ => self.label().to_owned(),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Timeout => "[Image: request timed out]",
            Self::Dns => "[Image: DNS resolution failed]",
            Self::Http(_) => "[Image: HTTP request failed]",
            Self::Response { reason, .. } => reason,
            Self::UnsafeUrl => "[Image: unsafe or non-HTTPS URL]",
            Self::Network => "[Image: download failed or timed out]",
            Self::AccessDenied => "[Image: access denied; open externally]",
            Self::NotFound => "[Image: no longer available]",
            Self::TooLarge => "[Image: exceeds preview size limit]",
            Self::Decode => "[Image: unsupported or invalid image data]",
            Self::Redirect => "[Image: too many or invalid redirects]",
            Self::Limit => "[Image: conversation preview limit reached]",
        }
    }
}

/// Strip markup and terminal controls from untrusted card labels.
pub fn plain(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        let name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if matches!(name, "br" | "p" | "div" | "li") {
            output.push('\n');
        } else if !matches!(
            name,
            "b" | "strong"
                | "i"
                | "em"
                | "s"
                | "strike"
                | "u"
                | "code"
                | "pre"
                | "a"
                | "font"
                | "ul"
                | "ol"
        ) {
            output.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    output
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}

pub fn styled(text: &str, base: Style) -> Line {
    let text = text
        .replace("<b>", "*")
        .replace("</b>", "*")
        .replace("<i>", "_")
        .replace("</i>", "_")
        .replace("<s>", "~")
        .replace("</s>", "~")
        .replace("<code>", "`")
        .replace("</code>", "`");
    let text = plain(&text);
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut style = base;
    let mut active = std::collections::BTreeSet::new();
    for (index, c) in text.char_indices() {
        let modifier = match c {
            '*' => Some(Modifier::BOLD),
            '_' => Some(Modifier::ITALIC),
            '~' => Some(Modifier::CROSSED_OUT),
            '`' => Some(Modifier::REVERSED),
            _ => None,
        };
        if let Some(modifier) =
            modifier.filter(|_| active.contains(&c) || text[index + c.len_utf8()..].contains(c))
        {
            spans.push(Span::styled(std::mem::take(&mut buffer), style));
            if active.remove(&c) {
                style = style.remove_modifier(modifier);
            } else {
                active.insert(c);
                style = style.add_modifier(modifier);
            }
        } else {
            buffer.push(c);
        }
    }
    spans.push(Span::styled(buffer, style));
    Line::from_spans(spans)
}

// Each redirect is resolved and pinned independently. No ambient proxies or
// credentials are used for external media, including Google CDN URLs.
fn image_url_allowed(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some()
    })
}

fn public_address(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..=127).contains(&b))
                && !(a == 198 && matches!(b, 18 | 19))
                && !(a == 192 && b == 0)
        }
        std::net::IpAddr::V6(ip) => {
            let segments = ip.segments();
            // Only global unicast; exclude transition/mapped and documentation ranges.
            (segments[0] & 0xe000) == 0x2000
                && segments[0] != 0x2002
                && !(segments[0] == 0x2001 && (segments[1] < 0x200 || segments[1] == 0xdb8))
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
    }
}

fn authenticated_media(url: &url::Url) -> bool {
    url.host_str() == Some("chat.googleapis.com")
        && url.path().starts_with("/v1/media/")
        && url.path().len() > "/v1/media/".len()
        && url.query() == Some("alt=media")
}

pub async fn load(urls: Vec<String>, auth: Option<Arc<crate::auth::AuthManager>>) -> Images {
    use futures_util::{StreamExt, stream};
    use tracing::Instrument;
    static NEXT_IMAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    stream::iter(urls.into_iter().enumerate().map(|(index, url)| {
        let auth = auth.clone();
        let image_id = NEXT_IMAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let span = tracing::info_span!(target: "gchatui::diagnostics", "image_load", image_id);
        async move {
            let started = std::time::Instant::now();
            tracing::info!(target: "gchatui::diagnostics", "image_started");
            let image = if index >= 16 {
                Err(ImageError::Limit)
            } else {
                tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    fetch(&url, auth.as_deref()),
                )
                .await
                .unwrap_or(Err(ImageError::Timeout))
                .map(Arc::new)
            };
            match &image {
                Ok(_) => tracing::info!(target: "gchatui::diagnostics", elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), "image_ready"),
                Err(error) => tracing::warn!(target: "gchatui::diagnostics", elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), failure = ?error, "image_failed"),
            }
            (url, image)
        }.instrument(span)
    }))
    .buffer_unordered(4)
    .collect()
    .await
}

const MAX_BYTES: usize = 16 * 1024 * 1024;

async fn fetch(
    value: &str,
    auth: Option<&crate::auth::AuthManager>,
) -> Result<DecodedImage, ImageError> {
    let mut url = url::Url::parse(value).map_err(|_| ImageError::UnsafeUrl)?;
    let mut may_authenticate = authenticated_media(&url);
    for redirect_count in 0..6 {
        tracing::info!(target: "gchatui::diagnostics", redirect_count, authenticated = may_authenticate, "image_request");
        if !image_url_allowed(url.as_str()) {
            return Err(ImageError::UnsafeUrl);
        }
        let host = url.host_str().ok_or(ImageError::UnsafeUrl)?;
        let addresses = tokio::net::lookup_host((host, 443))
            .await
            .map_err(|_| ImageError::Dns)?
            .collect::<Vec<_>>();
        if addresses.is_empty() || addresses.iter().any(|addr| !public_address(addr.ip())) {
            return Err(ImageError::UnsafeUrl);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &addresses)
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| ImageError::Network)?;
        let mut request = client.get(url.clone());
        let token = if may_authenticate && authenticated_media(&url) {
            Some(
                auth.ok_or(ImageError::AccessDenied)?
                    .access_token()
                    .await
                    .map_err(|_| ImageError::AccessDenied)?,
            )
        } else {
            None
        };
        if let Some(token) = &token {
            request = request.bearer_auth(token.as_str());
        }
        let mut response = request.send().await.map_err(|_| ImageError::Network)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            && let (Some(auth), Some(token)) = (auth, token.as_ref())
        {
            let replacement = auth
                .refresh_rejected_token(token)
                .await
                .map_err(|_| ImageError::AccessDenied)?;
            response = client
                .get(url.clone())
                .bearer_auth(replacement.as_str())
                .send()
                .await
                .map_err(|_| ImageError::Network)?;
        }
        tracing::info!(target: "gchatui::diagnostics", status = response.status().as_u16(), content_type = response_kind(response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("")), "image_response");
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or(ImageError::Redirect)?;
            url = url.join(location).map_err(|_| ImageError::Redirect)?;
            may_authenticate = false;
            continue;
        }
        match response.status().as_u16() {
            401 | 403 => return Err(ImageError::AccessDenied),
            404 | 410 => return Err(ImageError::NotFound),
            200..=299 => {}
            _ => return Err(ImageError::Http(response.status().as_u16())),
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(ImageError::TooLarge);
        }
        let status = response.status().as_u16();
        let kind = response_kind(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or(""),
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| ImageError::Network)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(ImageError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        tracing::info!(target: "gchatui::diagnostics", bytes = bytes.len(), "image_downloaded");
        return tokio::task::spawn_blocking(move || decode_response(&bytes, status, kind))
            .await
            .map_err(|_| ImageError::Decode)?;
    }
    Err(ImageError::Redirect)
}

// Only fixed categories enter diagnostics. Raw headers and decoder error text
// can contain server-controlled/private strings and must never be displayed.
fn response_kind(value: &str) -> &'static str {
    match value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "image/png" => "image/png",
        "image/jpeg" => "image/jpeg",
        "image/gif" => "image/gif",
        "image/webp" => "image/webp",
        "image/svg+xml" => "image/svg+xml",
        "text/html" => "text/html",
        "application/json" => "application/json",
        "application/octet-stream" => "application/octet-stream",
        "" => "missing",
        _ => "other",
    }
}

fn decode_response(
    bytes: &[u8],
    status: u16,
    kind: &'static str,
) -> Result<DecodedImage, ImageError> {
    let format = image::guess_format(bytes).ok();
    let format_name = match format {
        Some(image::ImageFormat::Png) => "PNG",
        Some(image::ImageFormat::Jpeg) => "JPEG",
        Some(image::ImageFormat::Gif) => "GIF",
        Some(image::ImageFormat::WebP) => "WebP",
        Some(_) => "other image format",
        None => "unrecognized",
    };
    let failure = |reason| ImageError::Response {
        status,
        kind,
        bytes: bytes.len(),
        format: format_name,
        reason,
    };
    if matches!(kind, "text/html" | "application/json") {
        return Err(failure("Server returned a web page or JSON, not an image."));
    }
    let Some(format) = format else {
        return Err(failure("Response has no recognized image signature."));
    };
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| {
            failure(match error {
                image::ImageError::Limits(_) => "Image exceeds decoder dimension or memory limits.",
                image::ImageError::Unsupported(_) => {
                    "Detected image format is not supported by this build."
                }
                image::ImageError::Decoding(_) => "Image decoder rejected malformed image data.",
                image::ImageError::IoError(_) => "Image data is truncated or unreadable.",
                _ => "Image decoder failed.",
            })
        })?
        .thumbnail(1024, 1024)
        .to_rgba8();
    let preview = image::imageops::thumbnail(&image, 320, 192);
    let payload = |image: image::RgbaImage| ImagePayload::Pixels {
        width: image.width(),
        height: image.height(),
        bytes: image.into_raw(),
        format: ImagePixelFormat::Rgba8,
    };
    Ok(DecodedImage {
        full: payload(image),
        preview: payload(preview),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_controls_and_styles_card_text() {
        assert_eq!(plain("<b>Hello</b>\x1b\x07"), "Hello");
        assert!(
            styled("<b>Hello</b>", Style::new())
                .spans
                .iter()
                .any(|s| s.content == "Hello" && s.style.modifiers.contains(Modifier::BOLD))
        );
    }
    #[test]
    fn image_sources_are_https_and_not_arbitrary_network_targets() {
        assert!(image_url_allowed(
            "https://example.googleusercontent.com/sample.png"
        ));
        assert!(image_url_allowed("https://example.com/sample.png"));
        for url in [
            "http://example.gstatic.com/a",
            "https://user:password@example.com/a",
            "file:///sample.png",
            "https://example.com:8443/a",
        ] {
            assert!(!image_url_allowed(url));
        }
    }
    #[test]
    fn private_and_special_networks_are_never_fetched() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "192.0.2.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn credentials_are_only_for_the_chat_media_endpoint() {
        assert!(authenticated_media(
            &url::Url::parse(
                "https://chat.googleapis.com/v1/media/spaces/sample/attachments/image?alt=media"
            )
            .unwrap()
        ));
        for value in [
            "https://example.com/v1/media/spaces/sample/attachments/image?alt=media",
            "https://chat.googleapis.com/v1/spaces/sample",
            "https://example.googleusercontent.com/image",
        ] {
            assert!(!authenticated_media(&url::Url::parse(value).unwrap()));
        }
    }

    #[tokio::test]
    async fn blocked_urls_return_specific_errors_without_network() {
        let images = load(vec!["file:///sample.png".into()], None).await;
        assert_eq!(images["file:///sample.png"], Err(ImageError::UnsafeUrl));
    }

    #[test]
    fn response_diagnostics_are_specific_and_never_echo_server_content() {
        let error = decode_response(
            b"<html>synthetic-private-marker</html>",
            200,
            response_kind("text/html; private=synthetic-private-marker"),
        )
        .unwrap_err();
        let details = error.details();
        assert!(details.contains("HTTP: 200"));
        assert!(details.contains("Content type: text/html"));
        assert!(details.contains("web page or JSON"));
        assert!(!details.contains("synthetic-private-marker"));
        assert_eq!(response_kind("synthetic-private-marker"), "other");
        let error = decode_response(&[0; 8], 200, "image/png").unwrap_err();
        assert!(error.details().contains("no recognized image signature"));
        assert!(error.details().contains("Received: 8 bytes"));
    }

    #[test]
    fn valid_and_oversized_images_have_distinct_outcomes() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(2, 2)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        assert!(decode_response(&bytes.into_inner(), 200, "image/png").is_ok());
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(4097, 1)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let error = decode_response(&bytes.into_inner(), 200, "image/png").unwrap_err();
        assert!(error.details().contains("dimension or memory limits"));
        assert!(error.details().contains("Detected format: PNG"));
    }

    #[test]
    fn decoded_cache_keeps_small_preview_and_full_expansion() {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(1024, 1024)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let decoded = decode_response(&bytes.into_inner(), 200, "image/png").unwrap();
        let ImagePayload::Pixels {
            width,
            height,
            bytes,
            ..
        } = decoded.preview
        else {
            panic!("pixel preview")
        };
        assert!(width <= 320 && height <= 192);
        assert!(bytes.len() <= 320 * 192 * 4);
        let ImagePayload::Pixels { width, height, .. } = decoded.full else {
            panic!("full pixels")
        };
        assert_eq!((width, height), (1024, 1024));
    }

    #[test]
    fn malformed_image_is_rejected() {
        assert!(decode_response(b"not an image", 200, "missing").is_err());
    }
}
