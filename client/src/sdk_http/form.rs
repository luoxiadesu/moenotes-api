use super::{SdkError, invalid};
use md5::{Digest, Md5};
use std::collections::BTreeMap;
use zeroize::Zeroizing;

// OkHttp 3.12.13 FormBody.addEncoded uses this set and preserves % and +.
const FORM_SET: &[u8] = b" \"':;<=>@[]^`{}|/\\?#&!$(),~";

pub(super) fn encoded(value: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::new());
    for &byte in value.as_bytes() {
        if matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r') {
            continue;
        }
        if !(32..127).contains(&byte) || FORM_SET.contains(&byte) {
            use std::fmt::Write;
            write!(out, "%{byte:02X}").unwrap();
        } else {
            out.push(byte as char);
        }
    }
    out
}

// Android Uri.encode(value) before addEncoded for alreadyEncoded=false requests.
pub(super) fn raw(value: &str) -> Zeroizing<String> {
    let mut uri = Zeroizing::new(String::new());
    for &byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"_-!.~'()*".contains(&byte) {
            uri.push(byte as char);
        } else {
            use std::fmt::Write;
            write!(uri, "%{byte:02X}").unwrap();
        }
    }
    encoded(&uri)
}

pub(super) fn decoded(value: &str) -> Result<Zeroizing<String>, SdkError> {
    let plus = Zeroizing::new(value.replace('+', " "));
    // Reject invalid UTF-8 instead of imitating Java replacement characters.
    let bytes = Zeroizing::new(percent_encoding::percent_decode_str(&plus).collect::<Vec<_>>());
    Ok(Zeroizing::new(
        std::str::from_utf8(&bytes)
            .map_err(|_| invalid())?
            .to_owned(),
    ))
}

pub(super) fn sign_form(
    fields: Vec<(String, Zeroizing<String>)>,
    key: &str,
) -> Result<Zeroizing<String>, SdkError> {
    let mut values = BTreeMap::new();
    for (name, value) in &fields {
        // Duplicate form fields remain on the wire, but the last value is signed.
        values.insert(name, decoded(value)?);
    }
    let mut digest = Md5::new();
    for (name, value) in values {
        if !name.eq_ignore_ascii_case("item_name") && !name.eq_ignore_ascii_case("item_desc") {
            digest.update(value.as_bytes());
        }
    }
    digest.update(key.as_bytes());
    let signature = Zeroizing::new(format!("{:x}", digest.finalize()));
    let mut body = Zeroizing::new(String::new());
    for (name, value) in fields {
        if !body.is_empty() {
            body.push('&');
        }
        body.push_str(&name);
        body.push('=');
        body.push_str(&value);
    }
    body.push_str("&sign=");
    body.push_str(&signature);
    if body.len() > 64 * 1024 {
        return Err(invalid());
    }
    Ok(body)
}
