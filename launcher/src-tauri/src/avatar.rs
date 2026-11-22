//! The player's avatar on the server: every account's shadNet `avatar_url` is
//! `http://fzh.avatar/a/<onlineId>.png`, which the game fetches through the host overrides from the
//! backend (port 1338). The launcher uploads the picture there with `PUT /a/<onlineId>.png`,
//! authorised by the shadNet token of the signed-in session (`GetToken`).

use crate::backend_api::{exchange, host_header, status_code};
use crate::error::{BackendError, Res};
use crate::instance::Server;

pub const SIDE: u32 = 256;
pub const MAX_BYTES: usize = 256 * 1024;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
const DATA_URL_PREFIX: &str = "data:image/png;base64,";

fn path(online_id: &str) -> String {
    format!("/a/{online_id}.png")
}

/// What the backend accepts (`valid_avatar` in server/backend/backend.py): a PNG, 256x256, at most 256 KB,
/// ending with its IEND chunk.
pub fn check_png(png: &[u8]) -> Res<()> {
    if png.len() > MAX_BYTES {
        return Err(BackendError::invalid("backend.avatar_too_large", "The picture is larger than 256 KB. Choose a simpler picture.")
            .with("size", crate::error::bytes(png.len() as u64)));
    }
    let ihdr = png.len() >= 33 && png.starts_with(PNG_MAGIC) && &png[12..16] == b"IHDR";
    if !ihdr || &png[png.len() - 8..png.len() - 4] != b"IEND" {
        return Err(BackendError::invalid("backend.avatar_not_png", "The avatar is not a PNG picture."));
    }
    let side = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    if (side(16), side(20)) != (SIDE, SIDE) {
        return Err(BackendError::invalid("backend.avatar_not_square", "The avatar must be 256×256 pixels."));
    }
    Ok(())
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &c in text {
        acc = acc << 6 | B64.iter().position(|&b| b == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// `Session.image` and `ProfileUpdate.image` carry the PNG as a data URL.
pub fn data_url(png: &[u8]) -> String {
    format!("{DATA_URL_PREFIX}{}", base64_encode(png))
}

pub fn from_data_url(url: &str) -> Res<Vec<u8>> {
    let png = url
        .strip_prefix(DATA_URL_PREFIX)
        .and_then(base64_decode)
        .ok_or_else(|| BackendError::invalid("backend.avatar_not_png", "The avatar is not a PNG picture."))?;
    check_png(&png)?;
    Ok(png)
}

fn put_request(host: &str, port: u16, online_id: &str, token: &str, png: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "PUT {} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path(online_id),
        host_header(host, port),
        png.len()
    )
    .into_bytes();
    out.extend_from_slice(png);
    out
}

fn status_error(status: Option<u16>) -> Option<BackendError> {
    match status {
        Some(200) => None,
        Some(403) => Some(BackendError::new("unauthorized", "backend.avatar_not_owner", "The server did not accept the avatar for this account.")),
        Some(400) => Some(BackendError::invalid("backend.avatar_refused", "The server refused the picture: it takes a 256×256 PNG of at most 256 KB.")),
        Some(code) => Some(BackendError::new("internal", "backend.avatar_server_error", format!("The server could not store the avatar (HTTP {code}).")).with("code", code)),
        None => Some(BackendError::new("internal", "backend.avatar_bad_reply", "The server gave an unexpected response to the avatar.")),
    }
}

fn unreachable() -> BackendError {
    BackendError::new("server_unreachable", "backend.avatar_unreachable", "Cannot reach the progress service to send the avatar.")
}

pub fn upload(server: &str, online_id: &str, token: &str, png: &[u8]) -> Res<()> {
    let at = Server::of(server);
    upload_to(&at.host, at.backend, online_id, token, png)
}

fn upload_to(host: &str, port: u16, online_id: &str, token: &str, png: &[u8]) -> Res<()> {
    check_png(png)?;
    let response = exchange(host, port, &put_request(host, port, online_id, token, png)).ok_or_else(unreachable)?;
    status_error(status_code(&response)).map_or(Ok(()), Err)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A valid 256x256 opaque RGB PNG of one colour.
    pub fn plain_png(rgb: [u8; 3]) -> Vec<u8> {
        use flate2::write::ZlibEncoder;
        let mut raw = Vec::new();
        for _ in 0..SIDE {
            raw.push(0);
            for _ in 0..SIDE {
                raw.extend_from_slice(&rgb);
            }
        }
        let mut z = ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&raw).unwrap();
        let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&crc.sum().to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&SIDE.to_be_bytes());
        ihdr.extend_from_slice(&SIDE.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        let mut png = PNG_MAGIC.to_vec();
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &z.finish().unwrap());
        chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn png_checks() {
        let png = plain_png([10, 20, 30]);
        assert_eq!(check_png(&png), Ok(()));
        assert_eq!(check_png(b"GIF89a").unwrap_err().key, "backend.avatar_not_png");
        assert_eq!(check_png(&png[..png.len() - 12]).unwrap_err().key, "backend.avatar_not_png");
        let mut wide = png.clone();
        wide[18] = 2;
        assert_eq!(check_png(&wide).unwrap_err().key, "backend.avatar_not_square");
        let mut big = png[..png.len() - 12].to_vec();
        big.resize(MAX_BYTES + 1, 0);
        big.extend_from_slice(&png[png.len() - 12..]);
        let e = check_png(&big).unwrap_err();
        assert_eq!((e.key, e.params["size"]["bytes"].as_u64()), ("backend.avatar_too_large", Some(MAX_BYTES as u64 + 13)));
    }

    #[test]
    fn base64_and_data_urls() {
        for (raw, text) in [(&b""[..], ""), (b"f", "Zg=="), (b"fo", "Zm8="), (b"foo", "Zm9v"), (b"\xff\xfe\x00", "//4A")] {
            assert_eq!(base64_encode(raw), text);
            assert_eq!(base64_decode(text).as_deref(), Some(raw));
        }
        assert_eq!(base64_decode("Zm9v!"), None);
        let png = plain_png([1, 2, 3]);
        let url = data_url(&png);
        assert!(url.starts_with("data:image/png;base64,iVBORw0KGgo"));
        assert_eq!(from_data_url(&url), Ok(png));
        assert_eq!(from_data_url("data:image/jpeg;base64,AAAA").unwrap_err().key, "backend.avatar_not_png");
    }

    #[test]
    fn put_request_and_replies() {
        let r = put_request("10.0.0.1", 1338, "Player_1", "tok", b"\x89PNG");
        let head = "PUT /a/Player_1.png HTTP/1.1\r\nHost: 10.0.0.1:1338\r\nAuthorization: Bearer tok\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\n";
        assert_eq!(r, [head.as_bytes(), b"\x89PNG"].concat());
        assert!(String::from_utf8_lossy(&put_request("fd00::1", 1338, "a_b", "t", b"")).contains("Host: [fd00::1]:1338\r\n"));
        assert!(status_error(Some(200)).is_none());
        assert_eq!(status_error(Some(403)).unwrap().code, "unauthorized");
        assert_eq!(status_error(Some(400)).unwrap().key, "backend.avatar_refused");
        assert_eq!(status_error(Some(500)).unwrap().params["code"], 500);
        assert_eq!(status_error(None).unwrap().key, "backend.avatar_bad_reply");
    }

    #[test]
    fn upload_over_a_fake_backend() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let png = plain_png([0, 0, 0]);
        let expected = put_request("127.0.0.1", port, "x_y", "t", &png);
        let server = std::thread::spawn(move || {
            for reply in ["HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n", "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut got = vec![0u8; expected.len()];
                s.read_exact(&mut got).unwrap();
                assert_eq!(got, expected);
                s.write_all(reply.as_bytes()).unwrap();
            }
        });
        assert_eq!(upload_to("127.0.0.1", port, "x_y", "t", &png), Ok(()));
        assert_eq!(upload_to("127.0.0.1", port, "x_y", "t", &png).unwrap_err().code, "unauthorized");
        server.join().unwrap();
        assert_eq!(upload_to("127.0.0.1", port, "x_y", "t", &png).unwrap_err().code, "server_unreachable");
        assert_eq!(upload_to("127.0.0.1", port, "x_y", "t", b"x").unwrap_err().code, "invalid_input");
    }
}
