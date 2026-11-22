//! Hand-written protobuf3 for the few shadNet messages the launcher uses (`shadnet.proto` of the
//! emulator fork). Default values are omitted on the wire, as protobuf3 does.

#[derive(Debug, Clone, PartialEq)]
pub struct ProtoError;

fn varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn put_str(out: &mut Vec<u8>, field: u32, s: &str) {
    if s.is_empty() {
        return;
    }
    varint(out, (u64::from(field) << 3) | 2);
    varint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

#[cfg(test)]
fn put_bool(out: &mut Vec<u8>, field: u32, b: bool) {
    if b {
        varint(out, u64::from(field) << 3);
        out.push(1);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed,
}

fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64, ProtoError> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos).ok_or(ProtoError)?;
        *pos += 1;
        n |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(n);
        }
    }
    Err(ProtoError)
}

fn fields(buf: &[u8]) -> Result<Vec<(u32, Field<'_>)>, ProtoError> {
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let number = u32::try_from(key >> 3).map_err(|_| ProtoError)?;
        let value = match key & 7 {
            0 => Field::Varint(read_varint(buf, &mut pos)?),
            1 | 5 => {
                let n = if key & 7 == 1 { 8 } else { 4 };
                pos = pos.checked_add(n).filter(|&p| p <= buf.len()).ok_or(ProtoError)?;
                Field::Fixed
            }
            2 => {
                let len = usize::try_from(read_varint(buf, &mut pos)?).map_err(|_| ProtoError)?;
                let end = pos.checked_add(len).filter(|&e| e <= buf.len()).ok_or(ProtoError)?;
                let bytes = &buf[pos..end];
                pos = end;
                Field::Bytes(bytes)
            }
            _ => return Err(ProtoError),
        };
        out.push((number, value));
    }
    Ok(out)
}

fn text(f: Field<'_>) -> Result<String, ProtoError> {
    match f {
        Field::Bytes(b) => String::from_utf8(b.to_vec()).map_err(|_| ProtoError),
        _ => Err(ProtoError),
    }
}

fn number(f: Field<'_>) -> Result<u64, ProtoError> {
    match f {
        Field::Varint(n) => Ok(n),
        _ => Err(ProtoError),
    }
}

pub fn login_request(npid: &str, password: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, npid);
    put_str(&mut out, 2, password);
    out
}

/// `GetAuthorizationCodeRequest {client_id = 1, scope = 2}`.
pub fn authorization_code_request(client_id: &str, scope: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, client_id);
    put_str(&mut out, 2, scope);
    out
}

pub fn friend_command(npid: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, npid);
    out
}

pub fn client_version(version: &str) -> Vec<u8> {
    let mut out = Vec::new();
    put_str(&mut out, 1, version);
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FriendEntry {
    pub npid: String,
    pub online: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoginReply {
    pub user_id: u64,
    pub friends: Vec<FriendEntry>,
    pub requests_sent: Vec<String>,
    pub requests_received: Vec<String>,
    pub blocked: Vec<String>,
}

impl LoginReply {
    pub fn decode(buf: &[u8]) -> Result<LoginReply, ProtoError> {
        let mut r = LoginReply::default();
        for (n, f) in fields(buf)? {
            match n {
                2 => r.user_id = number(f)?,
                3 => {
                    let Field::Bytes(inner) = f else { return Err(ProtoError) };
                    let mut e = FriendEntry::default();
                    for (m, g) in fields(inner)? {
                        match m {
                            1 => e.npid = text(g)?,
                            2 => e.online = number(g)? != 0,
                            _ => {}
                        }
                    }
                    r.friends.push(e);
                }
                4 => r.requests_sent.push(text(f)?),
                5 => r.requests_received.push(text(f)?),
                6 => r.blocked.push(text(f)?),
                _ => {}
            }
        }
        Ok(r)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenReply {
    pub token: String,
    pub user_id: u64,
    pub npid: String,
}

impl TokenReply {
    pub fn decode(buf: &[u8]) -> Result<TokenReply, ProtoError> {
        let mut r = TokenReply::default();
        for (n, f) in fields(buf)? {
            match n {
                1 => r.token = text(f)?,
                2 => r.user_id = number(f)?,
                3 => r.npid = text(f)?,
                _ => {}
            }
        }
        Ok(r)
    }
}

/// `GetAuthorizationCodeReply {code = 1, expires_in = 2}`; the code is single-use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuthorizationCodeReply {
    pub code: String,
    pub expires_in: u64,
}

impl AuthorizationCodeReply {
    pub fn decode(buf: &[u8]) -> Result<AuthorizationCodeReply, ProtoError> {
        let mut r = AuthorizationCodeReply::default();
        for (n, f) in fields(buf)? {
            match n {
                1 => r.code = text(f)?,
                2 => r.expires_in = number(f)?,
                _ => {}
            }
        }
        Ok(r)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notification {
    FriendQuery { from: String },
    FriendNew { npid: String, online: bool },
    FriendLost { npid: String },
    FriendStatus { npid: String, online: bool, timestamp_ns: u64 },
    ClientRejected { message: String },
    /// A player invited this account to a game session (WebAPI `POST /v1/sessions/<id>/invitations`).
    Invitation { from: String },
}

pub const FRIEND_QUERY: u16 = 5;
pub const FRIEND_NEW: u16 = 6;
pub const FRIEND_LOST: u16 = 7;
pub const FRIEND_STATUS: u16 = 8;
pub const WEB_API_PUSH_EVENT: u16 = 17;
pub const CLIENT_REJECTED: u16 = 19;

/// The invitation in a `WebApiPushEvent` payload, which is not protobuf: blobs (u32 LE size and
/// bytes) for the service name, a u32 label, then blobs for data type, data, from and to, then
/// extended data the launcher does not read (`ClientSession::BuildWebApiPushPayload`).
pub fn invitation(payload: &[u8]) -> Option<Notification> {
    fn take<'a>(buf: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
        let bytes = buf.get(*pos..pos.checked_add(n)?)?;
        *pos += n;
        Some(bytes)
    }
    fn blob<'a>(buf: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
        let size = u32::from_le_bytes(take(buf, pos, 4)?.try_into().ok()?) as usize;
        take(buf, pos, size)
    }
    let mut pos = 0;
    let service = blob(payload, &mut pos)?;
    take(payload, &mut pos, 4)?;
    let data_type = blob(payload, &mut pos)?;
    blob(payload, &mut pos)?;
    let from = String::from_utf8(blob(payload, &mut pos)?.to_vec()).ok()?;
    let wanted = service == b"sessionInvitation" && data_type == b"np:service:invitation" && !from.is_empty();
    wanted.then_some(Notification::Invitation { from })
}

impl Notification {
    /// `None` for notification types the launcher does not use.
    pub fn decode(kind: u16, proto: &[u8]) -> Result<Option<Notification>, ProtoError> {
        if ![FRIEND_QUERY, FRIEND_NEW, FRIEND_LOST, FRIEND_STATUS, CLIENT_REJECTED].contains(&kind) {
            return Ok(None);
        }
        let (mut s1, mut s2, mut flag, mut ts) = (String::new(), String::new(), false, 0u64);
        for (n, f) in fields(proto)? {
            match (kind, n) {
                (CLIENT_REJECTED, 2) => s2 = text(f)?,
                (CLIENT_REJECTED, _) => {}
                (_, 1) => s1 = text(f)?,
                (FRIEND_NEW | FRIEND_STATUS, 2) => flag = number(f)? != 0,
                (FRIEND_STATUS, 3) => ts = number(f)?,
                _ => {}
            }
        }
        Ok(Some(match kind {
            FRIEND_QUERY => Notification::FriendQuery { from: s1 },
            FRIEND_NEW => Notification::FriendNew { npid: s1, online: flag },
            FRIEND_LOST => Notification::FriendLost { npid: s1 },
            FRIEND_STATUS => Notification::FriendStatus { npid: s1, online: flag, timestamp_ns: ts },
            _ => Notification::ClientRejected { message: s2 },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        let mut v = Vec::new();
        varint(&mut v, 300);
        assert_eq!(v, [0xac, 0x02]);
        let mut p = 0;
        assert_eq!(read_varint(&v, &mut p), Ok(300));
        assert!(read_varint(&[0x80], &mut 0).is_err());
    }

    #[test]
    fn empty_fields_are_omitted() {
        assert_eq!(authorization_code_request("ab", ""), [0x0a, 0x02, b'a', b'b']);
        assert_eq!(friend_command(""), Vec::<u8>::new());
    }

    #[test]
    fn authorization_code_reply() {
        let mut buf = vec![0x0a, 4, b'C', b'0', b'd', b'e', 0x10, 0xac, 0x02];
        assert_eq!(AuthorizationCodeReply::decode(&buf), Ok(AuthorizationCodeReply { code: "C0de".into(), expires_in: 300 }));
        buf.extend([0x1a, 1, b'x']);
        assert_eq!(AuthorizationCodeReply::decode(&buf).unwrap().code, "C0de");
        assert_eq!(AuthorizationCodeReply::decode(&[]), Ok(AuthorizationCodeReply::default()));
        assert!(AuthorizationCodeReply::decode(&[0x08, 0x05]).is_err());
        assert!(AuthorizationCodeReply::decode(&[0x10, 0x80]).is_err());
    }

    #[test]
    fn token_reply() {
        let mut buf = Vec::new();
        put_str(&mut buf, 1, "abc.def");
        varint(&mut buf, 2 << 3);
        varint(&mut buf, 7);
        put_str(&mut buf, 3, "Player_1");
        buf.extend([0x25, 1, 2, 3, 4]);
        assert_eq!(TokenReply::decode(&buf), Ok(TokenReply { token: "abc.def".into(), user_id: 7, npid: "Player_1".into() }));
        assert_eq!(TokenReply::decode(&[]), Ok(TokenReply::default()));
        assert!(TokenReply::decode(&[0x08, 0x05]).is_err());
        assert!(TokenReply::decode(&[0x0a, 0x05, b'a']).is_err());
    }

    fn entry(npid: &str, online: bool) -> Vec<u8> {
        let mut e = Vec::new();
        put_str(&mut e, 1, npid);
        put_bool(&mut e, 2, online);
        let mut out = Vec::new();
        varint(&mut out, (3 << 3) | 2);
        varint(&mut out, e.len() as u64);
        out.extend(e);
        out
    }

    #[test]
    fn login_reply_decodes_all_lists() {
        let mut buf = Vec::new();
        put_str(&mut buf, 1, "http://avatar");
        varint(&mut buf, 2 << 3);
        varint(&mut buf, 42);
        buf.extend(entry("Alpha", true));
        buf.extend(entry("Beta", false));
        put_str(&mut buf, 4, "Gamma");
        put_str(&mut buf, 5, "Delta");
        put_str(&mut buf, 5, "Echo");
        put_str(&mut buf, 6, "Foxtrot");
        buf.extend([0x3d, 1, 2, 3, 4]);
        let r = LoginReply::decode(&buf).unwrap();
        assert_eq!(r.user_id, 42);
        assert_eq!(
            r.friends,
            vec![FriendEntry { npid: "Alpha".into(), online: true }, FriendEntry { npid: "Beta".into(), online: false }]
        );
        assert_eq!(r.requests_sent, ["Gamma"]);
        assert_eq!(r.requests_received, ["Delta", "Echo"]);
        assert_eq!(r.blocked, ["Foxtrot"]);
        assert_eq!(LoginReply::decode(&[]).unwrap(), LoginReply::default());
        assert!(LoginReply::decode(&[0x22, 0x05, b'a']).is_err());
    }

    #[test]
    fn notifications() {
        let mut b = Vec::new();
        put_str(&mut b, 1, "Kestrel");
        assert_eq!(Notification::decode(FRIEND_QUERY, &b), Ok(Some(Notification::FriendQuery { from: "Kestrel".into() })));
        assert_eq!(Notification::decode(FRIEND_LOST, &b), Ok(Some(Notification::FriendLost { npid: "Kestrel".into() })));
        put_bool(&mut b, 2, true);
        assert_eq!(
            Notification::decode(FRIEND_NEW, &b),
            Ok(Some(Notification::FriendNew { npid: "Kestrel".into(), online: true }))
        );
        varint(&mut b, 3 << 3);
        varint(&mut b, 1_700_000_000_000_000_000);
        assert_eq!(
            Notification::decode(FRIEND_STATUS, &b),
            Ok(Some(Notification::FriendStatus { npid: "Kestrel".into(), online: true, timestamp_ns: 1_700_000_000_000_000_000 }))
        );
        let mut r = Vec::new();
        varint(&mut r, 1 << 3);
        varint(&mut r, 1);
        put_str(&mut r, 2, "Please update.");
        put_str(&mut r, 3, "x");
        assert_eq!(
            Notification::decode(CLIENT_REJECTED, &r),
            Ok(Some(Notification::ClientRejected { message: "Please update.".into() }))
        );
        assert_eq!(Notification::decode(17, &[0xff]), Ok(None));
    }

    /// As `ClientSession::BuildWebApiPushPayload` of shadNet b38cf175.
    fn push(service: &str, data_type: &str, data: &[u8], from: &str) -> Vec<u8> {
        let blob = |b: &[u8]| [&(b.len() as u32).to_le_bytes()[..], b].concat();
        let mut p = blob(service.as_bytes());
        p.extend(0u32.to_le_bytes());
        for b in [data_type.as_bytes(), data, from.as_bytes(), b"Me".as_slice()] {
            p.extend(blob(b));
        }
        p.extend(1u32.to_le_bytes());
        p.extend(blob(b"sessionId"));
        p.extend(blob(b"s-1"));
        p.extend(7u64.to_le_bytes());
        p.extend(9u64.to_le_bytes());
        p
    }

    #[test]
    fn invitation_push() {
        let body = br#"{"invitationId":"002-x","fromUser":{"onlineId":"Kestrel"}}"#;
        let p = push("sessionInvitation", "np:service:invitation", body, "Kestrel");
        assert_eq!(invitation(&p), Some(Notification::Invitation { from: "Kestrel".into() }));
        assert_eq!(invitation(&push("sessionInvitation", "np:service:session:game:join:to:member", b"", "Kestrel")), None);
        assert_eq!(invitation(&push("", "np:service:presence:onlineStatus", b"", "Kestrel")), None);
        assert_eq!(invitation(&push("sessionInvitation", "np:service:invitation", body, "")), None);
        assert_eq!(invitation(&p[..30]), None);
        assert_eq!(invitation(&[]), None);
    }
}
