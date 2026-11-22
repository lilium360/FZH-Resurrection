//! Packet framing of the shadNet binary protocol (`documentation/protocol.md`): a 15-byte
//! little-endian header, then the payload; protobuf payloads are "proto blobs" (u32 LE size + bytes).

pub const HEADER_SIZE: usize = 15;
pub const MAX_PACKET_SIZE: u32 = 0x80_0000;
pub const PROTOCOL_VERSION: u32 = 1;

pub const REQUEST: u8 = 0;
pub const REPLY: u8 = 1;
pub const NOTIFICATION: u8 = 2;
pub const SERVER_INFO: u8 = 3;

pub const LOGIN: u16 = 0;
pub const TERMINATE: u16 = 1;
pub const ADD_FRIEND: u16 = 8;
pub const REMOVE_FRIEND: u16 = 9;
pub const SET_CLIENT_VERSION: u16 = 13;
pub const GET_TOKEN: u16 = 39;
pub const GET_AUTHORIZATION_CODE: u16 = 42;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Header {
    pub kind: u8,
    pub command: u16,
    pub size: u32,
    pub id: u64,
}

impl Header {
    pub fn parse(b: &[u8; HEADER_SIZE]) -> Header {
        Header {
            kind: b[0],
            command: u16::from_le_bytes([b[1], b[2]]),
            size: u32::from_le_bytes([b[3], b[4], b[5], b[6]]),
            id: u64::from_le_bytes(b[7..15].try_into().unwrap()),
        }
    }

    pub fn payload_len(&self) -> Option<usize> {
        if (self.size as usize) < HEADER_SIZE || self.size > MAX_PACKET_SIZE {
            return None;
        }
        Some(self.size as usize - HEADER_SIZE)
    }
}

pub fn request(command: u16, id: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.push(REQUEST);
    out.extend_from_slice(&command.to_le_bytes());
    out.extend_from_slice(&((HEADER_SIZE + payload.len()) as u32).to_le_bytes());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn blob(proto: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + proto.len());
    out.extend_from_slice(&(proto.len() as u32).to_le_bytes());
    out.extend_from_slice(proto);
    out
}

/// The protobuf bytes of a proto blob at the start of `payload`.
pub fn read_blob(payload: &[u8]) -> Option<&[u8]> {
    let size = u32::from_le_bytes(payload.get(..4)?.try_into().ok()?) as usize;
    payload.get(4..4usize.checked_add(size)?)
}

/// The protocol version of a ServerInfo packet, or `None` when the packet is not one.
pub fn server_info_version(header: &Header, payload: &[u8]) -> Option<u32> {
    if header.kind != SERVER_INFO {
        return None;
    }
    Some(u32::from_le_bytes(payload.get(..4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shadnet::proto;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// The play grant's request (packet id 1): the header of `protocol.md`, then the proto blob that
    /// `CmdGetAuthorizationCode` (shadNet `cmd_account.cpp`) reads.
    #[test]
    fn authorization_code_request_bytes() {
        let expected = hex(concat!(
            "002a002a0000000100000000000000",
            "17000000",
            "0a0b",
            "667a682d6261636b656e64",
            "1208",
            "667a683a706c6179",
        ));
        let msg = proto::authorization_code_request("fzh-backend", "fzh:play");
        assert_eq!(request(GET_AUTHORIZATION_CODE, 1, &blob(&msg)), expected);
    }

    /// The reply: header, the error byte, then a proto blob (`code`, `expires_in` 300).
    #[test]
    fn authorization_code_reply_bytes() {
        let b = hex("012a00210000000100000000000000000d0000000a08416243644566676810ac02");
        let h = Header::parse(b[..HEADER_SIZE].try_into().unwrap());
        assert_eq!((h.kind, h.command, h.payload_len()), (REPLY, GET_AUTHORIZATION_CODE, Some(b.len() - HEADER_SIZE)));
        let (error, rest) = b[HEADER_SIZE..].split_first().unwrap();
        assert_eq!(*error, 0);
        let reply = proto::AuthorizationCodeReply::decode(read_blob(rest).unwrap()).unwrap();
        assert_eq!((reply.code.as_str(), reply.expires_in), ("AbCdEfgh", 300));
    }

    #[test]
    fn server_info_from_the_protocol_document() {
        let b = hex("03000013000000000000000000000001000000");
        let h = Header::parse(b[..HEADER_SIZE].try_into().unwrap());
        assert_eq!(h.payload_len(), Some(4));
        assert_eq!(server_info_version(&h, &b[HEADER_SIZE..]), Some(1));
        assert_eq!(server_info_version(&h, &b[HEADER_SIZE..18]), None);
        assert_eq!(server_info_version(&Header { kind: REPLY, ..h }, &b[HEADER_SIZE..]), None);
    }

    #[test]
    fn header_and_blob() {
        let p = request(TERMINATE, 0x0102030405060708, &[]);
        assert_eq!(p, hex("0001000f0000000807060504030201"));
        let h = Header::parse(p[..15].try_into().unwrap());
        assert_eq!(h, Header { kind: REQUEST, command: TERMINATE, size: 15, id: 0x0102030405060708 });
        assert_eq!(h.payload_len(), Some(0));
        assert_eq!(Header { size: 3, ..h }.payload_len(), None);
        assert_eq!(Header { size: MAX_PACKET_SIZE + 1, ..h }.payload_len(), None);
        assert_eq!(read_blob(&[2, 0, 0, 0, 9, 8, 7]), Some(&[9u8, 8][..]));
        assert_eq!(read_blob(&[5, 0, 0, 0, 9]), None);
        assert_eq!(read_blob(&[1, 0]), None);
    }
}
