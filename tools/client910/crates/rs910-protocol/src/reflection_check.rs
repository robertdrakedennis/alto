//! The server's `REFLECTION_CHECKER` request and the
//! `REFLECTION_CHECK_REPLY` written while the client is in game state 18.
//!
//! The reference client resolves each request against its own VM with a
//! class lookup by name. This client has no VM, so a name resolves exactly
//! when that lookup cannot depend on the reference client's own classes: the
//! primitive descriptors, platform (bootstrap-loader) classes and arrays of
//! either. A platform class has no class loader, so an owner lookup against
//! it is refused by the security check and the entry records
//! [`STATUS_SECURITY`]. Every other owner is one of the reference client's own
//! classes.
//! TODO(#reflection-client-classes): the reference answers those from its live
//! static fields/methods; they do not exist here, so the lookup reports the
//! class-not-found result ([`STATUS_CLASS_NOT_FOUND`]).
use crate::payload_reader::PayloadReader;

/// The package prefixes of platform classes, one per line (wire vocabulary of
/// the server's request names).
const PLATFORM_PACKAGES: &str = include_str!("reflection_platform_packages.txt");

/// Reply status of a lookup whose class was not found.
pub const STATUS_CLASS_NOT_FOUND: i8 = -1;
/// Reply status of a lookup against a platform class (refused by the security check).
pub const STATUS_SECURITY: i8 = -2;
/// Reply status of an unknown operation (an absent member).
pub const STATUS_ABSENT_MEMBER: i8 = -19;

/// One queued reflection check request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Request id echoed first in the reply.
    pub id: i32,
    pub entries: Vec<Entry>,
}

/// One check slot: the operation and its decode status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// 0 getInt, 1 setInt, 2 field modifiers, 3 invoke, 4 method modifiers.
    /// Unknown operations leave it at the default 0.
    pub op: u8,
    /// 0 when the lookup succeeded, else the negative exception code
    /// recorded by `decode`.
    pub status: i8,
}

/// True when the class lookup would find the name here, which is always a class
/// without a class loader; false is the class-not-found result.
fn resolves(name: &str) -> bool {
    if matches!(name, "B" | "I" | "S" | "J" | "Z" | "F" | "D" | "C" | "void") {
        return true;
    }
    // The lookup accepts VM array descriptors ("[I", "[L<class>;").
    if let Some(element) = name.strip_prefix('[') {
        let element = element.trim_start_matches('[');
        return match element.as_bytes().first() {
            Some(b'B' | b'I' | b'S' | b'J' | b'Z' | b'F' | b'D' | b'C') => element.len() == 1,
            Some(b'L') => element
                .strip_prefix('L')
                .and_then(|e| e.strip_suffix(';'))
                .is_some_and(resolves),
            _ => false,
        };
    }
    // Platform (bootstrap-loaded) packages.
    // TODO(#reflection-client-classes): see module docs.
    PLATFORM_PACKAGES.lines().any(|p| name.starts_with(p)) && !name.ends_with('.')
}

/// Owner lookup for `getDeclaredField`/`getDeclaredMethods`: -1 when the class
/// is not found, [`STATUS_SECURITY`] for a bootstrap class.
fn owner_status(owner: &str) -> i8 {
    if resolves(owner) {
        STATUS_SECURITY
    } else {
        STATUS_CLASS_NOT_FOUND
    }
}

/// Decode a `REFLECTION_CHECKER` payload.
pub fn decode(payload: &[u8]) -> anyhow::Result<Check> {
    let mut r = PayloadReader::new(payload);
    let count = r.g1()?;
    let id = r.g4s()?;
    let mut entries = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        let op = r.g1()?;
        let entry = match op {
            0..=2 => {
                let owner = r.gjstr()?;
                let _field = r.gjstr()?;
                if op == 1 {
                    let _value = r.g4s()?;
                }
                Entry {
                    op,
                    status: owner_status(&owner),
                }
            }
            3 | 4 => {
                let owner = r.gjstr()?;
                let _method = r.gjstr()?;
                let arity = r.g1()?;
                let params = (0..arity)
                    .map(|_| r.gjstr())
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let ret = r.gjstr()?;
                if op == 3 {
                    for _ in 0..arity {
                        let len = r.g4s()?;
                        let len = usize::try_from(len).map_err(|_| {
                            anyhow::anyhow!("REFLECTION_CHECKER argument length {len}")
                        })?;
                        anyhow::ensure!(
                            len <= r.remaining(),
                            "REFLECTION_CHECKER argument length {len}"
                        );
                        r.pos += len;
                    }
                }
                // Parameter classes, then the return class, then the owner;
                // the first failure wins.
                let status = if params.iter().chain([&ret]).any(|n| !resolves(n)) {
                    STATUS_CLASS_NOT_FOUND
                } else {
                    owner_status(&owner)
                };
                Entry { op, status }
            }
            // Unknown operations read nothing more and keep status 0.
            _ => Entry { op: 0, status: 0 },
        };
        entries.push(entry);
    }
    Ok(Check { id, entries })
}

/// Encode the reply: opcode, `psize1`,
/// `p4(id)`, one result per entry, then `addcrc` over the id and results.
#[must_use]
pub fn encode_reply(check: &Check) -> Vec<u8> {
    let mut body = check.id.to_be_bytes().to_vec();
    for entry in &check.entries {
        if entry.status != 0 {
            body.push(entry.status as u8);
            continue;
        }
        // Status 0 with no resolved member: only an unknown operation
        // reaches here (op 0 against a null member), which is answered with
        // the absent-member code.
        debug_assert_eq!(entry.op, 0);
        body.push(STATUS_ABSENT_MEMBER as u8);
    }
    let crc = rs910_core::checksum::crc32(&body);
    body.extend_from_slice(&crc.to_be_bytes());
    let mut out = Vec::with_capacity(body.len() + 2);
    out.push(crate::proto::client::REFLECTION_CHECK_REPLY);
    out.push(u8::try_from(body.len()).unwrap_or(u8::MAX));
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A class name under the first platform package.
    fn platform(rest: &str) -> String {
        format!("{}{rest}", PLATFORM_PACKAGES.lines().next().unwrap())
    }

    fn s(out: &mut Vec<u8>, text: &str) {
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }

    #[test]
    fn decodes_and_replies_with_status_codes() {
        let mut p = vec![5];
        p.extend(0x1234_5678i32.to_be_bytes());
        // op 0 on a platform owner: refused by the security check (-2).
        p.push(0);
        s(&mut p, &platform("lang.System"));
        s(&mut p, "out");
        // op 1 on a client class: class not found (-1).
        p.push(1);
        s(&mut p, "client");
        s(&mut p, "field");
        p.extend(7i32.to_be_bytes());
        // op 3 with an unknown parameter class: -1 before the owner check.
        p.push(3);
        s(&mut p, &platform("lang.Math"));
        s(&mut p, "abs");
        p.push(1);
        s(&mut p, "nope.Type");
        s(&mut p, "I");
        p.extend(2i32.to_be_bytes());
        p.extend([0xac, 0xed]);
        // op 4 with primitive signature on a platform owner: -2.
        p.push(4);
        s(&mut p, &platform("lang.Math"));
        s(&mut p, "abs");
        p.push(1);
        s(&mut p, "I");
        s(&mut p, "I");
        // Unknown op: nothing else read, reply absent member (-19).
        p.push(9);
        let check = decode(&p).unwrap();
        assert_eq!(check.id, 0x1234_5678);
        let statuses: Vec<_> = check.entries.iter().map(|e| e.status).collect();
        assert_eq!(statuses, [-2, -1, -1, -2, 0]);
        let reply = encode_reply(&check);
        let body = [0x12, 0x34, 0x56, 0x78, 0xfe, 0xff, 0xff, 0xfe, 0xed];
        let crc = rs910_core::checksum::crc32(&body).to_be_bytes();
        let mut expected = vec![crate::proto::client::REFLECTION_CHECK_REPLY, 13];
        expected.extend(body);
        expected.extend(crc);
        assert_eq!(reply, expected);
    }

    #[test]
    fn resolves_primitive_array_and_bootstrap_names() {
        assert!(resolves("void"));
        assert!(resolves("[[I"));
        assert!(resolves(&format!("[L{};", platform("lang.String"))));
        assert!(!resolves("[Lclient;"));
        assert!(!resolves("client"));
        assert!(decode(&[1, 0, 0, 0, 1, 0, b'x']).is_err());
    }
}
